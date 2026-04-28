use meerkat_integration_tests::e2e_lanes::{run_catalog_scenario, run_named_suite};

macro_rules! smoke_scenario {
    ($name:ident, $id:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "lane:e2e-smoke"]
        async fn $name() {
            run_catalog_scenario($id)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    };
}

macro_rules! smoke_suite {
    ($name:ident, $suite:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "lane:e2e-smoke"]
        async fn $name() {
            run_named_suite($suite)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    };
}

macro_rules! smoke_entry {
    (scenario, $name:ident, $id:literal) => {
        smoke_scenario!($name, $id);
    };
    (suite, $name:ident, $suite:literal) => {
        smoke_suite!($name, $suite);
    };
}

macro_rules! smoke_tests {
    ($($kind:ident($name:ident, $value:tt);)*) => {
        $(smoke_entry!($kind, $name, $value);)*
    };
}

meerkat_integration_tests::e2e_smoke_lane_entries!(smoke_tests);
