/// The server that answers for leftover rules and new versions.
// shortcut: a release build must set RID_SERVER; without it this is the server of a local test set-up.
pub const SERVER: &str = match option_env!("RID_SERVER") {
    Some(address) => address,
    None => "http://127.0.0.1:8080",
};

pub mod cmdline;
pub mod driver;
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
pub mod update;
