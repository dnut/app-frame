/// Enables logging of errors, to move forward without returning the error.
pub trait LogError<T>: Sized {
    ////////////////////////////////////////
    // Converts to option, logged as error

    /// Logs if there was an error and converts the result into an option
    fn log(self) -> Option<T> {
        self.log_as(tracing::log::Level::Error)
    }
    /// Logs if there was an error with a message and converts the result into an option
    fn log_context(self, ctx: &str) -> Option<T> {
        self.log_context_as(tracing::log::Level::Error, ctx)
    }
    /// Lazily logs if there was an error with a message and converts the result into an option
    fn log_with_context<Ctx: Fn() -> String>(self, ctx: Ctx) -> Option<T> {
        self.log_with_context_as(tracing::log::Level::Error, ctx)
    }

    ////////////////////////////////////////
    // Converts to option, with customizable log level

    /// Logs if there was an error and converts the result into an option
    fn log_as(self, level: tracing::log::Level) -> Option<T>;
    /// Logs if there was an error with a message at the provided log level, and converts the result into an option
    fn log_context_as(self, level: tracing::log::Level, ctx: &str) -> Option<T> {
        self.log_with_context_as(level, || ctx.into())
    }
    /// Lazily logs if there was an error with a message at the provided log level, and converts the result into an option
    fn log_with_context_as<Ctx: Fn() -> String>(
        self,
        level: tracing::log::Level,
        ctx: Ctx,
    ) -> Option<T>;

    ////////////////////////////////////////
    // Passthrough functions that do not change the result, logged as error

    /// Logs if there was an error and returns the result unchanged
    fn log_passthrough(self) -> Self {
        self.log_as_passthrough(tracing::log::Level::Error)
    }
    /// Logs if there was an error with a message and returns the result unchanged
    fn log_context_passthrough(self, ctx: &str) -> Self {
        self.log_context_as_passthrough(tracing::log::Level::Error, ctx)
    }
    /// Lazily logs if there was an error with a message and returns the result unchanged
    fn log_with_context_passthrough<Ctx: Fn() -> String>(self, ctx: Ctx) -> Self {
        self.log_with_context_as_passthrough(tracing::log::Level::Error, ctx)
    }

    ////////////////////////////////////////
    // Passthrough functions that do not change the result, with customizable log level

    /// Logs if there was an error and returns the result unchanged
    fn log_as_passthrough(self, level: tracing::log::Level) -> Self;
    /// Logs if there was an error with a message at the provided log level, and returns the result unchanged
    fn log_context_as_passthrough(self, level: tracing::log::Level, ctx: &str) -> Self {
        self.log_with_context_as_passthrough(level, || ctx.into())
    }
    /// Lazily logs if there was an error with a message at the provided log level, and returns the result unchanged
    fn log_with_context_as_passthrough<Ctx: Fn() -> String>(
        self,
        level: tracing::log::Level,
        ctx: Ctx,
    ) -> Self;
}

impl<T, E: std::fmt::Display + 'static> LogError<T> for Result<T, E> {
    fn log_as(self, level: tracing::log::Level) -> Option<T> {
        self.log_as_passthrough(level).ok()
    }

    fn log_with_context_as<Ctx: Fn() -> String>(
        self,
        level: tracing::log::Level,
        ctx: Ctx,
    ) -> Option<T> {
        self.log_with_context_as_passthrough(level, ctx).ok()
    }

    fn log_as_passthrough(self, level: tracing::log::Level) -> Self {
        self.map_err(|e| {
            let es = display_error(&e);
            log!(level, "{es}");
            e
        })
    }

    fn log_with_context_as_passthrough<Ctx: Fn() -> String>(
        self,
        level: tracing::log::Level,
        ctx: Ctx,
    ) -> Self {
        self.map_err(|e| {
            let ctx = ctx();
            let es = display_error(&e);
            log!(level, "error: `{ctx}` - {es}");
            e
        })
    }
}

macro_rules! log {
    ($level:expr, $($args:tt),*) => {
        match $level {
            tracing::log::Level::Error => tracing::error!($($args),*),
            tracing::log::Level::Warn => tracing::warn!($($args),*),
            tracing::log::Level::Info => tracing::info!($($args),*),
            tracing::log::Level::Debug => tracing::debug!($($args),*),
            tracing::log::Level::Trace => tracing::trace!($($args),*),
        };
    };
}
pub(crate) use log;

/// use this to make sure you have a descriptive message including a stack trace
/// for anyhow errors, and otherwise just display the normal string for other
/// errors.
pub fn display_error<E: std::fmt::Display + 'static>(e: &E) -> String {
    match (e as &dyn std::any::Any).downcast_ref::<anyhow::Error>() {
        Some(nehau) => {
            let mut s = String::new();
            format_anyhow(nehau, &mut s).unwrap();
            s
        }
        None => format!("{e}"),
    }
}

fn format_anyhow<W: std::fmt::Write>(e: &anyhow::Error, f: &mut W) -> std::fmt::Result {
    write!(f, "{}", e)?;
    for i in e.chain().skip(1) {
        write!(f, ", caused by: {}", i)?;
    }
    write!(f, "\nstack backtrace:\n{}", e.backtrace())
}
