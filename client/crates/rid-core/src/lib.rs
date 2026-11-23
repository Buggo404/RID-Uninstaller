/// The server that answers for leftover rules and new versions.
/// RID_SERVER at build time points a build at another one (a local test set-up, say).
pub const SERVER: &str = match option_env!("RID_SERVER") {
    Some(address) if !address.is_empty() => address,
    _ => "https://rid.bitheart.top",
};

pub mod ai;
pub mod cmdline;
pub mod driver;
pub mod facts;
pub mod icons;
pub mod inventory;
pub mod links;
pub mod msi;
pub mod names;
pub mod ops;
pub mod paths;
pub mod procs;
pub mod rules;
pub mod safety;
pub mod scan;
pub mod shell;
pub mod sources;
pub mod startup;
pub mod tasks;
pub mod trace;
pub mod traced;
#[cfg(test)]
mod testutil;
pub mod uninstall;
