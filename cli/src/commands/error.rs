use std::fmt::Display;

pub struct CommandError {
    pub error: anyhow::Error,
    pub hints: Vec<String>,
}

pub type CommandResult<T> = Result<T, CommandError>;

pub trait Hint<T> {
    fn hint<C: Display>(self, hint: C) -> Result<T, CommandError>;

    fn with_hint<F, H>(self, hint_fn: F) -> Result<T, CommandError>
    where
        F: FnOnce() -> H,
        H: Display;
}

impl<T> Hint<T> for Result<T, anyhow::Error> {
    fn hint<C: Display>(self, hint: C) -> Result<T, CommandError> {
        self.map_err(|error| CommandError {
            error,
            hints: vec![hint.to_string()],
        })
    }

    fn with_hint<F, H>(self, hint_fn: F) -> Result<T, CommandError>
    where
        F: FnOnce() -> H,
        H: Display,
    {
        self.map_err(|error| CommandError {
            error,
            hints: vec![hint_fn().to_string()],
        })
    }
}

impl<T> Hint<T> for Result<T, CommandError> {
    fn hint<C: Display>(self, hint: C) -> Result<T, CommandError> {
        self.map_err(|mut error| {
            error.hints.push(hint.to_string());
            error
        })
    }

    fn with_hint<F, H>(self, hint_fn: F) -> Result<T, CommandError>
    where
        F: FnOnce() -> H,
        H: Display,
    {
        self.map_err(|mut error| {
            error.hints.push(hint_fn().to_string());
            error
        })
    }
}

impl<E> From<E> for CommandError
where
    anyhow::Error: From<E>,
{
    fn from(error: E) -> Self {
        Self {
            error: error.into(),
            hints: Vec::new(),
        }
    }
}

macro_rules! write_error {
    ($dst:expr, $($arg:tt)*) => {
        writeln!($dst, "{}: {}", colored::Colorize::bright_red("Error"), format_args!($($arg)*))
    };
}

macro_rules! write_hint {
    ($dst:expr, $($arg:tt)*) => {
        writeln!($dst, "{}: {}", colored::Colorize::bright_blue(" Hint"), format_args!($($arg)*))
    };
}

impl Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_error!(f, "{}", self.error)?;
        for hint in self.hints.iter() {
            write_hint!(f, "{}", hint)?;
        }
        Ok(())
    }
}
