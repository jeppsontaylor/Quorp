use std::fmt::{Display, Formatter};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Importance {
    Critical,
    Important,
    #[default]
    Average,
    Iffy,
    Fluff,
}

impl Display for Importance {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Importance::Critical => "critical",
            Importance::Important => "important",
            Importance::Average => "average",
            Importance::Iffy => "iffy",
            Importance::Fluff => "fluff",
        };
        formatter.write_str(value)
    }
}

pub mod consts {
    pub const SUF_NORMAL: &str = "__perf_main";
    pub const SUF_MDATA: &str = "__perf_metadata";
    pub const ITER_ENV_VAR: &str = "PERF_ITERATIONS";

    pub const WEIGHT_DEFAULT: usize = 50;

    pub const MDATA_LINE_PREF: &str = "perf-meta";
    pub const ITER_COUNT_LINE_NAME: &str = "iterations";
    pub const WEIGHT_LINE_NAME: &str = "weight";
    pub const IMPORTANCE_LINE_NAME: &str = "importance";
    pub const VERSION_LINE_NAME: &str = "version";
    pub const MDATA_VER: usize = 1;
}
