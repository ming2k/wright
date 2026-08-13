// Test harness output (diagnostics on failure) goes through the raw print
// macros by design; the print_stdout/print_stderr lints guard CLI code only.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod integration {
    mod build_test;
    mod diversion_test;
    mod force_test;
    mod install_pipeline_test;
    mod install_test;
    mod isolation_test;
    mod launch_test;

    mod migration_test;
    mod output_test;
    mod plan_snapshot_test;
}
