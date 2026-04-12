use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};
use std::path::Path;
use std::sync::LazyLock;
use tracing::info;

/// Global multi-progress coordinator.  Every progress bar should be registered
/// through this instance so `indicatif` can manage terminal lines without
/// flickering.  Log output is routed here via [`MultiProgressWriter`] so that
/// `tracing` lines are inserted above active progress bars.
pub static MULTI: LazyLock<MultiProgress> = LazyLock::new(MultiProgress::new);

fn source_prefix(label: &str) -> String {
    format!("fetch {}", label)
}

pub fn source_label(uri: &str) -> String {
    let uri = uri.strip_prefix("git+").unwrap_or(uri);
    let uri = uri.strip_prefix("file://").unwrap_or(uri);
    let uri = uri.split('#').next().unwrap_or(uri);
    let tail = uri
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(uri);
    tail.trim_end_matches(".git").to_string()
}

pub fn new_source_transfer_bar(label: &str, total: u64) -> ProgressBar {
    let pb = MULTI.add(ProgressBar::new(total));
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{prefix} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {msg}")
            .expect("valid source transfer template")
            .progress_chars("#>-"),
    );
    pb.set_prefix(source_prefix(label));
    pb
}

pub fn set_source_bytes(pb: &ProgressBar, transferred: u64, total: u64) {
    pb.set_length(total);
    pb.set_position(transferred);
    if total > 0 {
        pb.set_message(format!(
            "{} / {}",
            HumanBytes(transferred),
            HumanBytes(total)
        ));
    } else {
        pb.set_message(format!("{}", HumanBytes(transferred)));
    }
}

pub fn set_source_git_objects(pb: &ProgressBar, received: u64, total: u64, bytes: u64) {
    pb.set_length(total);
    pb.set_position(received);
    pb.set_message(format!(
        "{}/{} objects, {}",
        received,
        total,
        HumanBytes(bytes)
    ));
}

pub fn new_source_spinner(label: &str, action: &str) -> ProgressBar {
    let pb = MULTI.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {prefix} [{elapsed_precise}] {msg}")
            .expect("valid source spinner template"),
    );
    pb.set_prefix(source_prefix(label));
    pb.set_message(action.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

pub fn finish_source(pb: &ProgressBar, _label: &str, dest: &Path) {
    pb.finish_and_clear();
    let filename = dest
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| dest.to_string_lossy().into_owned());
    info!("Fetched {}", filename);
}

/// A [`std::io::Write`] adapter that routes every complete line through
/// [`MULTI.println`] so `tracing` output does not overwrite progress bars.
pub struct MultiProgressWriter;

impl std::io::Write for MultiProgressWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        let _ = MULTI.println(s.trim_end());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for MultiProgressWriter {
    type Writer = MultiProgressWriter;

    fn make_writer(&'a self) -> Self::Writer {
        MultiProgressWriter
    }
}
