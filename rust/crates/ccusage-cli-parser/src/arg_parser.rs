use std::ffi::OsString;

pub(crate) struct ArgParser {
    pub(crate) args: Vec<String>,
    index: usize,
    flags: Vec<String>,
    pub(crate) timezone: Option<String>,
    inline_value: Option<InlineValue>,
}

struct InlineValue {
    flag: String,
    value: String,
}

impl ArgParser {
    pub(crate) fn new(args: Vec<OsString>) -> Result<Self, String> {
        let mut parsed = Vec::with_capacity(args.len());
        for arg in args {
            parsed.push(
                arg.into_string()
                    .map_err(|_| "Arguments must be valid UTF-8".to_string())?,
            );
        }
        Ok(Self {
            args: parsed,
            index: 0,
            flags: Vec::new(),
            timezone: None,
            inline_value: None,
        })
    }

    pub(crate) fn peek(&self) -> Option<&str> {
        self.args.get(self.index).map(String::as_str)
    }

    pub(crate) fn peek_name(&self) -> Option<&str> {
        self.peek()
            .map(|arg| arg.split_once('=').map_or(arg, |(name, _)| name))
    }

    pub(crate) fn next(&mut self) -> Option<String> {
        let value = self.args.get(self.index)?.clone();
        self.index += 1;
        Some(value)
    }

    pub(crate) fn saw_flag(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|flag| names.contains(&flag.as_str()))
    }

    pub(crate) fn next_flag(&mut self) -> Result<String, String> {
        self.reject_unread_inline_value()?;
        let arg = self
            .next()
            .ok_or_else(|| "Expected option but reached end of arguments".to_string())?;
        if let Some((flag, value)) = arg.split_once('=') {
            if !flag.starts_with('-') {
                return Err(format!("Expected option, got '{arg}'"));
            }
            self.inline_value = Some(InlineValue {
                flag: flag.to_string(),
                value: value.to_string(),
            });
            self.flags.push(flag.to_string());
            return Ok(flag.to_string());
        }
        if arg.starts_with('-') {
            self.flags.push(arg.clone());
            Ok(arg)
        } else {
            Err(format!("Expected option, got '{arg}'"))
        }
    }

    pub(crate) fn value_for(&mut self, flag: &str) -> Result<String, String> {
        if let Some(InlineValue { value, .. }) = self.inline_value.take() {
            if value.is_empty() {
                return Err(format!("Missing value for {flag}"));
            }
            return Ok(value);
        }
        let value = self
            .next()
            .ok_or_else(|| format!("Missing value for {flag}"))?;
        if value.starts_with('-') {
            return Err(format!("Missing value for {flag}"));
        }
        Ok(value)
    }

    pub(crate) fn read_timezone(&mut self) -> Result<(), String> {
        self.timezone = Some(self.value_for("--timezone")?);
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<(), String> {
        self.reject_unread_inline_value()?;
        match self.next() {
            Some(extra) => Err(format!("Unexpected argument '{extra}'")),
            None => Ok(()),
        }
    }

    fn reject_unread_inline_value(&mut self) -> Result<(), String> {
        match self.inline_value.take() {
            Some(InlineValue { flag, .. }) => Err(format!("{flag} does not take a value")),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn parser(args: &[&str]) -> ArgParser {
        ArgParser::new(args.iter().map(OsString::from).collect()).unwrap()
    }

    #[test]
    fn rejects_an_inline_value_the_flag_did_not_read() {
        let mut parser = parser(&["--offline=false", "--since", "20260101"]);

        assert_eq!(parser.next_flag().unwrap(), "--offline");
        assert_eq!(
            parser.next_flag().unwrap_err(),
            "--offline does not take a value"
        );
    }

    #[test]
    fn rejects_an_unread_inline_value_on_the_last_flag() {
        let mut parser = parser(&["--offline=false"]);

        assert_eq!(parser.next_flag().unwrap(), "--offline");
        assert_eq!(
            parser.finish().unwrap_err(),
            "--offline does not take a value"
        );
    }

    #[test]
    fn reads_an_inline_value_once() {
        let mut parser = parser(&["--since=20260101", "--json"]);

        assert_eq!(parser.next_flag().unwrap(), "--since");
        assert_eq!(parser.value_for("--since").unwrap(), "20260101");
        assert_eq!(parser.next_flag().unwrap(), "--json");
        assert!(parser.finish().is_ok());
    }

    #[test]
    fn rejects_inline_values_on_non_options() {
        let mut parser = parser(&["daily=bad"]);

        assert_eq!(
            parser.next_flag().unwrap_err(),
            "Expected option, got 'daily=bad'"
        );
    }
}
