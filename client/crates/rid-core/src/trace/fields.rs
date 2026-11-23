//! Reads the fields of an ETW event by name, using the layout the provider's manifest declares.

use std::sync::Arc;

use windows::Win32::System::Diagnostics::Etw::{EVENT_HEADER_FLAG_32_BIT_HEADER, EVENT_RECORD, PropertyParamCount, PropertyStruct, TRACE_EVENT_INFO, TdhGetEventInformation};

#[derive(Debug)]
pub enum Value {
    Number(u64),
    Text(String),
}

/// Field names with their TDH input types, in the order they lie in the event's data.
pub type Layout = Arc<Vec<(String, u16)>>;

/// The layout of this kind of event. Stops at the first field that is not a plain value;
/// everything this program reads comes before any such field.
pub fn layout(record: &EVENT_RECORD) -> Option<Layout> {
    let mut size = 0u32;
    // SAFETY: the first call only asks for the size; the buffer of the second has that size
    // and is aligned for the structure. Offsets inside it are checked against its length.
    unsafe {
        TdhGetEventInformation(record, None, None, &mut size);
        if (size as usize) < size_of::<TRACE_EVENT_INFO>() {
            return None;
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let info = buf.as_mut_ptr().cast::<TRACE_EVENT_INFO>();
        if TdhGetEventInformation(record, None, Some(info), &mut size) != 0 {
            return None;
        }
        let bytes = std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size as usize);
        let first = (*info).EventPropertyInfoArray.as_ptr();
        let mut fields = Vec::new();
        for i in 0..(*info).TopLevelPropertyCount as usize {
            let property = &*first.add(i);
            if property.Flags.0 & (PropertyStruct.0 | PropertyParamCount.0) != 0 {
                break;
            }
            let name = wide(bytes.get(property.NameOffset as usize..)?).0;
            fields.push((name, property.Anonymous1.nonStructType.InType));
        }
        Some(Arc::new(fields))
    }
}

/// A NUL-terminated UTF-16 string and the bytes it occupies, terminator included.
fn wide(bytes: &[u8]) -> (String, usize) {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    (String::from_utf16_lossy(&units), ((units.len() + 1) * 2).min(bytes.len()))
}

/// The values of an event, parallel to its layout; shorter if the data ends early or a field
/// is of a kind not read here.
pub fn values(layout: &[(String, u16)], record: &EVENT_RECORD) -> Vec<Value> {
    let pointer = if record.EventHeader.Flags & EVENT_HEADER_FLAG_32_BIT_HEADER as u16 != 0 { 4 } else { 8 };
    // SAFETY: ETW hands over UserDataLength readable bytes at UserData for the duration of the callback.
    let data = unsafe { std::slice::from_raw_parts(record.UserData as *const u8, record.UserDataLength as usize) };
    let mut out = Vec::with_capacity(layout.len());
    let mut at = 0;
    for (_, in_type) in layout {
        let rest = data.get(at..).unwrap_or_default();
        let number = |len: usize| {
            let bytes = rest.get(..len)?;
            let mut le = [0u8; 8];
            le[..len.min(8)].copy_from_slice(&bytes[..len.min(8)]);
            Some((Value::Number(u64::from_le_bytes(le)), len))
        };
        // TDH_INTYPE_*: strings, integers of each width, float/double, boolean, GUID, pointer, FILETIME, SID, hex integers.
        let read = match in_type {
            1 => Some(wide(rest)).map(|(text, used)| (Value::Text(text), used)),
            2 => Some(rest.iter().position(|&b| b == 0).unwrap_or(rest.len())).map(|end| (Value::Text(String::from_utf8_lossy(&rest[..end]).into_owned()), (end + 1).min(rest.len()))),
            3 | 4 => number(1),
            5 | 6 => number(2),
            7 | 8 | 11 | 13 | 20 => number(4),
            9 | 10 | 12 | 17 | 21 => number(8),
            15 => number(16),
            16 => number(pointer),
            19 => rest.get(1).and_then(|&sub_authorities| number(8 + 4 * sub_authorities as usize)),
            _ => None,
        };
        let Some((value, used)) = read else { break };
        out.push(value);
        at += used;
    }
    out
}
