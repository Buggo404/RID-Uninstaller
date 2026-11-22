//! Scheduled tasks that start a program when a user logs on, through the Task Scheduler service.

use std::io;

use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ILogonTrigger, IRegisteredTask, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TASK_CREATE_OR_UPDATE,
    TASK_ENUM_HIDDEN, TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_HIGHEST, TASK_RUNLEVEL_LUA, TASK_TRIGGER_LOGON, TaskScheduler,
};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4};
use windows::core::{BSTR, Interface};

use crate::links::Com;

fn int(n: i32) -> VARIANT {
    let mut v = VARIANT::default();
    // SAFETY: writes the tag and the union member that belongs to it.
    unsafe {
        (*v.Anonymous.Anonymous).vt = VT_I4;
        (*v.Anonymous.Anonymous).Anonymous.lVal = n;
    }
    v
}

fn text(s: &str) -> VARIANT {
    let mut v = VARIANT::default();
    // SAFETY: as above; the string is owned by the VARIANT from here on.
    unsafe {
        (*v.Anonymous.Anonymous).vt = VT_BSTR;
        (*v.Anonymous.Anonymous).Anonymous.bstrVal = std::mem::ManuallyDrop::new(BSTR::from(s));
    }
    v
}

#[derive(Debug, Clone)]
pub struct LogonTask {
    /// Full path, e.g. `\Vendor\Updater`.
    pub path: String,
    pub name: String,
    pub enabled: bool,
    pub exe: String,
    pub arguments: String,
    pub author: String,
}

fn fail(e: windows::core::Error) -> io::Error {
    io::Error::other(e.message())
}

fn connect() -> windows::core::Result<ITaskService> {
    // SAFETY: standard COM activation; the caller holds a `Com` guard for this thread.
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let none = VARIANT::default();
        service.Connect(&none, &none, &none, &none)?;
        Ok(service)
    }
}

/// The first program the task runs, if it is triggered at logon.
fn logon_task(task: &IRegisteredTask) -> windows::core::Result<Option<LogonTask>> {
    // SAFETY: COM calls on interfaces obtained from the service; collections are 1-based.
    unsafe {
        let definition = task.Definition()?;
        let triggers = definition.Triggers()?;
        let mut at_logon = false;
        let mut count = 0;
        triggers.Count(&mut count)?;
        for i in 1..=count {
            let mut kind = Default::default();
            triggers.get_Item(i)?.Type(&mut kind)?;
            at_logon |= kind == TASK_TRIGGER_LOGON;
        }
        if !at_logon {
            return Ok(None);
        }
        let actions = definition.Actions()?;
        actions.Count(&mut count)?;
        for i in 1..=count {
            let action = actions.get_Item(i)?;
            let mut kind = Default::default();
            action.Type(&mut kind)?;
            if kind != TASK_ACTION_EXEC {
                continue;
            }
            let exec: IExecAction = action.cast()?;
            let (mut exe, mut arguments, mut author) = (BSTR::new(), BSTR::new(), BSTR::new());
            exec.Path(&mut exe)?;
            let _ = exec.Arguments(&mut arguments);
            let _ = definition.RegistrationInfo().and_then(|info| info.Author(&mut author));
            return Ok(Some(LogonTask {
                path: task.Path()?.to_string(),
                name: task.Name()?.to_string(),
                enabled: task.Enabled()?.as_bool(),
                exe: exe.to_string(),
                arguments: arguments.to_string(),
                author: author.to_string(),
            }));
        }
        Ok(None)
    }
}

fn walk(folder: &ITaskFolder, out: &mut Vec<LogonTask>) -> windows::core::Result<()> {
    // SAFETY: as above.
    unsafe {
        // Windows' own maintenance tasks are not startup programs.
        if folder.Path()?.to_string().to_lowercase().starts_with(r"\microsoft") {
            return Ok(());
        }
        let tasks = folder.GetTasks(TASK_ENUM_HIDDEN.0)?;
        for i in 1..=tasks.Count()? {
            // One unreadable task (broken XML, no access) must not hide the others.
            if let Ok(Some(task)) = tasks.get_Item(&int(i)).and_then(|t| logon_task(&t)) {
                out.push(task);
            }
        }
        let folders = folder.GetFolders(0)?;
        for i in 1..=folders.Count()? {
            walk(&folders.get_Item(&int(i))?, out)?;
        }
        Ok(())
    }
}

pub fn logon_tasks() -> io::Result<Vec<LogonTask>> {
    let _com = Com::init();
    let mut out = Vec::new();
    // SAFETY: as above.
    unsafe { connect().and_then(|s| s.GetFolder(&BSTR::from("\\"))).and_then(|root| walk(&root, &mut out)) }.map_err(fail)?;
    Ok(out)
}

fn split(path: &str) -> io::Result<(&str, &str)> {
    match path.rsplit_once('\\') {
        Some((folder, name)) if path.starts_with('\\') && !name.is_empty() => Ok((if folder.is_empty() { "\\" } else { folder }, name)),
        _ => Err(io::Error::other("not a task path")),
    }
}

pub fn set_enabled(path: &str, enabled: bool) -> io::Result<()> {
    let (folder, name) = split(path)?;
    let _com = Com::init();
    // SAFETY: as above.
    unsafe {
        connect()
            .and_then(|s| s.GetFolder(&BSTR::from(folder)))
            .and_then(|f| f.GetTask(&BSTR::from(name)))
            .and_then(|t| t.SetEnabled(VARIANT_BOOL::from(enabled)))
    }
    .map_err(fail)
}

pub fn delete(path: &str) -> io::Result<()> {
    let (folder, name) = split(path)?;
    let _com = Com::init();
    // SAFETY: as above.
    unsafe { connect().and_then(|s| s.GetFolder(&BSTR::from(folder))).and_then(|f| f.DeleteTask(&BSTR::from(name), 0)) }
        .map_err(fail)
}

/// Registers `\<name>` to run `exe` when the current user logs on.
pub fn create(name: &str, exe: &str, arguments: &str, highest_privileges: bool) -> io::Result<()> {
    if name.is_empty() || name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(io::Error::other("not a valid task name"));
    }
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").map_err(io::Error::other)?
    );
    let _com = Com::init();
    // SAFETY: as above.
    unsafe {
        (|| {
            let service = connect()?;
            let definition = service.NewTask(0)?;
            let principal = definition.Principal()?;
            principal.SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)?;
            principal.SetRunLevel(if highest_privileges { TASK_RUNLEVEL_HIGHEST } else { TASK_RUNLEVEL_LUA })?;
            let settings = definition.Settings()?;
            settings.SetDisallowStartIfOnBatteries(VARIANT_BOOL::from(false))?;
            settings.SetStopIfGoingOnBatteries(VARIANT_BOOL::from(false))?;
            settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;
            let trigger: ILogonTrigger = definition.Triggers()?.Create(TASK_TRIGGER_LOGON)?.cast()?;
            trigger.SetUserId(&BSTR::from(user.as_str()))?;
            let action: IExecAction = definition.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
            action.SetPath(&BSTR::from(exe))?;
            action.SetArguments(&BSTR::from(arguments))?;
            service.GetFolder(&BSTR::from("\\"))?.RegisterTaskDefinition(
                &BSTR::from(name),
                &definition,
                TASK_CREATE_OR_UPDATE.0,
                &text(&user),
                &VARIANT::default(),
                TASK_LOGON_INTERACTIVE_TOKEN,
                &VARIANT::default(),
            )?;
            Ok(())
        })()
    }
    .map_err(fail)
}
