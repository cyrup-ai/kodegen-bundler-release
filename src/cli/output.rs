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
    pub fn info(&self, message: &str) {
        if self.quiet {
            return;
        }

        let mut buffer = self.bufwtr.buffer();
        
        // Try colored output to stdout
        if buffer.set_color(ColorSpec::new().set_fg(Some(Color::Cyan))).is_err()
            || write!(&mut buffer, "ℹ ").is_err()
            || buffer.reset().is_err()
            || writeln!(&mut buffer, " {}", message).is_err()
            || self.bufwtr.print(&buffer).is_err()
        {
            // Fallback: plain text to stderr
            eprintln!("[OUTPUT ERROR] ℹ  {}", message);
        }
    }

    /// Print a success message
    pub fn success(&self, message: &str) {
        if self.quiet {
            return;
        }

        let mut buffer = self.bufwtr.buffer();
        
        // Try colored output to stdout
        if buffer.set_color(ColorSpec::new().set_fg(Some(Color::Green)).set_bold(true)).is_err()
            || write!(&mut buffer, "✓").is_err()
            || buffer.reset().is_err()
            || writeln!(&mut buffer, " {}", message).is_err()
            || self.bufwtr.print(&buffer).is_err()
        {
            // Fallback: plain text to stderr
            eprintln!("[OUTPUT ERROR] ✓ {}", message);
        }
    }

    /// Print a warning message
    pub fn warn(&self, message: &str) {
        if self.quiet {
            return;
        }

        let mut buffer = self.bufwtr.buffer();
        
        // Try colored output to stdout
        if buffer.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)).set_bold(true)).is_err()
            || write!(&mut buffer, "⚠").is_err()
            || buffer.reset().is_err()
            || buffer.set_color(ColorSpec::new().set_fg(Some(Color::Yellow))).is_err()
            || writeln!(&mut buffer, " {}", message).is_err()
            || buffer.reset().is_err()
            || self.bufwtr.print(&buffer).is_err()
        {
            // Fallback: plain text to stderr
            eprintln!("[OUTPUT ERROR] ⚠ {}", message);
        }
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
    pub fn verbose(&self, message: &str) {
        if !self.verbose || self.quiet {
            return;
        }

        let mut buffer = self.bufwtr.buffer();
        
        // Try colored output to stdout
        if buffer.set_color(ColorSpec::new().set_fg(Some(Color::Blue))).is_err()
            || write!(&mut buffer, "→").is_err()
            || buffer.reset().is_err()
            || buffer.set_color(ColorSpec::new().set_fg(Some(Color::White))).is_err()
            || writeln!(&mut buffer, " {}", message).is_err()
            || buffer.reset().is_err()
            || self.bufwtr.print(&buffer).is_err()
        {
            // Fallback: plain text to stderr
            eprintln!("[OUTPUT ERROR] → {}", message);
        }
    }

    /// Print a progress message with spinner/activity indicator
    pub fn progress(&self, message: &str) {
        if self.quiet {
            return;
        }

        let mut buffer = self.bufwtr.buffer();
        
        // Try colored output to stdout
        if buffer.set_color(ColorSpec::new().set_fg(Some(Color::Magenta))).is_err()
            || write!(&mut buffer, "⋯").is_err()
            || buffer.reset().is_err()
            || writeln!(&mut buffer, " {}", message).is_err()
            || self.bufwtr.print(&buffer).is_err()
        {
            // Fallback: plain text to stderr
            eprintln!("[OUTPUT ERROR] ⋯ {}", message);
        }
    }

    /// Print a section header
    pub fn section(&self, title: &str) {
        if self.quiet {
            return;
        }

        let mut buffer = self.bufwtr.buffer();
        
        // Try colored output to stdout
        if writeln!(&mut buffer).is_err()
            || buffer.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)).set_bold(true)).is_err()
            || writeln!(&mut buffer, "═══ {} ═══", title).is_err()
            || buffer.reset().is_err()
            || self.bufwtr.print(&buffer).is_err()
        {
            // Fallback: plain text to stderr
            eprintln!("[OUTPUT ERROR] \n═══ {} ═══", title);
        }
    }

    /// Print indented text (for sub-items)
    pub fn indent(&self, message: &str) {
        if self.quiet {
            return;
        }

        let mut buffer = self.bufwtr.buffer();
        
        // Try output to stdout
        if writeln!(&mut buffer, "    {}", message).is_err()
            || self.bufwtr.print(&buffer).is_err()
        {
            // Fallback: plain text to stderr
            eprintln!("[OUTPUT ERROR]     {}", message);
        }
    }

    /// Print a plain message (respects quiet mode)
    pub fn println(&self, message: &str) {
        if self.quiet {
            return;
        }

        let mut buffer = self.bufwtr.buffer();
        
        // Try output to stdout
        if writeln!(&mut buffer, "{}", message).is_err()
            || self.bufwtr.print(&buffer).is_err()
        {
            // Fallback: plain text to stderr
            eprintln!("[OUTPUT ERROR] {}", message);
        }
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
