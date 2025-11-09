//! Colored terminal output for release operations
//!
//! Provides consistent, colored CLI output with proper formatting

#![allow(dead_code)] // Public API - methods may be used by external consumers

use std::io::Write;
use termcolor::{BufferWriter, Color, ColorChoice, ColorSpec, WriteColor};

/// Output manager for consistent colored terminal output
#[derive(Debug)]
pub struct OutputManager {
    bufwtr: BufferWriter,
    verbose: bool,
    quiet: bool,
}

impl Clone for OutputManager {
    fn clone(&self) -> Self {
        Self {
            bufwtr: BufferWriter::stdout(ColorChoice::Auto),
            verbose: self.verbose,
            quiet: self.quiet,
        }
    }
}

impl OutputManager {
    /// Create a new output manager
    pub fn new(verbose: bool, quiet: bool) -> Self {
        Self {
            bufwtr: BufferWriter::stdout(ColorChoice::Auto),
            verbose,
            quiet,
        }
    }

    /// Print an info message (normal output)
    pub fn info(&self, message: &str) -> std::io::Result<()> {
        if self.quiet {
            return Ok(());
        }

        let mut buffer = self.bufwtr.buffer();
        let _ = buffer.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)));
        let _ = write!(&mut buffer, "ℹ ");
        let _ = buffer.reset();
        let _ = writeln!(&mut buffer, " {}", message);
        self.bufwtr.print(&buffer)
    }

    /// Print a success message
    pub fn success(&self, message: &str) -> std::io::Result<()> {
        if self.quiet {
            return Ok(());
        }

        let mut buffer = self.bufwtr.buffer();
        let _ = buffer.set_color(ColorSpec::new().set_fg(Some(Color::Green)).set_bold(true));
        let _ = write!(&mut buffer, "✓");
        let _ = buffer.reset();
        let _ = writeln!(&mut buffer, " {}", message);
        self.bufwtr.print(&buffer)
    }

    /// Print a warning message
    pub fn warn(&self, message: &str) -> std::io::Result<()> {
        if self.quiet {
            return Ok(());
        }

        let mut buffer = self.bufwtr.buffer();
        let _ = buffer.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)).set_bold(true));
        let _ = write!(&mut buffer, "⚠");
        let _ = buffer.reset();
        let _ = buffer.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)));
        let _ = writeln!(&mut buffer, " {}", message);
        let _ = buffer.reset();
        self.bufwtr.print(&buffer)
    }

    /// Print an error message (always shown)
    pub fn error(&self, message: &str) {
        let bufwtr = BufferWriter::stderr(ColorChoice::Auto);
        let mut buffer = bufwtr.buffer();

        // Try colored output to stderr
        if buffer.set_color(ColorSpec::new().set_fg(Some(Color::Red)).set_bold(true)).is_err()
            || write!(&mut buffer, "✗").is_err()
            || buffer.reset().is_err()
            || buffer.set_color(ColorSpec::new().set_fg(Some(Color::Red))).is_err()
            || writeln!(&mut buffer, " {}", message).is_err()
            || buffer.reset().is_err()
            || bufwtr.print(&buffer).is_err()
        {
            // Stderr failed - fallback to stdout as last resort
            println!("[STDERR ERROR] ✗ {}", message);
        }
    }

    /// Print a verbose/debug message (only in verbose mode)
    pub fn verbose(&self, message: &str) -> std::io::Result<()> {
        if !self.verbose || self.quiet {
            return Ok(());
        }

        let mut buffer = self.bufwtr.buffer();
        let _ = buffer.set_color(ColorSpec::new().set_fg(Some(Color::Blue)));
        let _ = write!(&mut buffer, "→");
        let _ = buffer.reset();
        let _ = buffer.set_color(ColorSpec::new().set_fg(Some(Color::White)));
        let _ = writeln!(&mut buffer, " {}", message);
        let _ = buffer.reset();
        self.bufwtr.print(&buffer)
    }

    /// Print a progress message with spinner/activity indicator
    pub fn progress(&self, message: &str) -> std::io::Result<()> {
        if self.quiet {
            return Ok(());
        }

        let mut buffer = self.bufwtr.buffer();
        let _ = buffer.set_color(ColorSpec::new().set_fg(Some(Color::Magenta)));
        let _ = write!(&mut buffer, "⋯");
        let _ = buffer.reset();
        let _ = writeln!(&mut buffer, " {}", message);
        self.bufwtr.print(&buffer)
    }

    /// Print a section header
    pub fn section(&self, title: &str) -> std::io::Result<()> {
        if self.quiet {
            return Ok(());
        }

        let mut buffer = self.bufwtr.buffer();
        let _ = writeln!(&mut buffer);
        let _ = buffer.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)).set_bold(true));
        let _ = writeln!(&mut buffer, "═══ {} ═══", title);
        let _ = buffer.reset();
        self.bufwtr.print(&buffer)
    }

    /// Print indented text (for sub-items)
    pub fn indent(&self, message: &str) -> std::io::Result<()> {
        if self.quiet {
            return Ok(());
        }

        let mut buffer = self.bufwtr.buffer();
        let _ = writeln!(&mut buffer, "    {}", message);
        self.bufwtr.print(&buffer)
    }

    /// Print a plain message (respects quiet mode)
    pub fn println(&self, message: &str) -> std::io::Result<()> {
        if self.quiet {
            return Ok(());
        }

        let mut buffer = self.bufwtr.buffer();
        let _ = writeln!(&mut buffer, "{}", message);
        self.bufwtr.print(&buffer)
    }

    /// Check if verbose mode is enabled
    pub fn is_verbose(&self) -> bool {
        self.verbose
    }

    /// Check if quiet mode is enabled
    pub fn is_quiet(&self) -> bool {
        self.quiet
    }
}
