//! Guest kernel operations behind the platform boundary.

mod linux;

pub(crate) use linux::run;
