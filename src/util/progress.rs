use indicatif::MultiProgress;
use std::sync::LazyLock;

/// Global multi-progress coordinator.  Every progress bar should be registered
/// through this instance so `indicatif` can manage terminal lines without
/// flickering.  Log output is routed here via [`MultiProgressWriter`] so that
/// `tracing` lines are inserted above active progress bars.
pub static MULTI: LazyLock<MultiProgress> = LazyLock::new(MultiProgress::new);

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
