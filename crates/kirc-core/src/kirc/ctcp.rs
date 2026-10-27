#[derive(Debug, PartialEq)]
pub(super) enum CtcpCommand {
    Version,
    Ping(String),
    Time,
    Unknown(String),
}

pub(super) fn parse_ctcp(message: &str) -> Option<CtcpCommand> {
    if !message.starts_with('\x01') || !message.ends_with('\x01') {
        return None;
    }

    let inner = &message[1..message.len() - 1];
    if inner.is_empty() {
        return None;
    }

    let mut parts = inner.splitn(2, ' ');

    let cmd = parts.next()?.to_uppercase();
    let arg = parts.next().unwrap_or("").to_string();

    match cmd.as_str() {
        "VERSION" => Some(CtcpCommand::Version),
        "PING" => Some(CtcpCommand::Ping(arg)),
        "TIME" => Some(CtcpCommand::Time),
        _ => Some(CtcpCommand::Unknown(inner.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ctcp_version() {
        assert_eq!(parse_ctcp("\x01VERSION\x01"), Some(CtcpCommand::Version));
    }

    #[test]
    fn test_parse_ctcp_ping() {
        assert_eq!(
            parse_ctcp("\x01PING 123456\x01"),
            Some(CtcpCommand::Ping("123456".to_string()))
        );
    }

    #[test]
    fn test_parse_ctcp_time() {
        assert_eq!(parse_ctcp("\x01TIME\x01"), Some(CtcpCommand::Time));
    }

    #[test]
    fn test_parse_ctcp_unknown() {
        assert_eq!(
            parse_ctcp("\x01FOO bar\x01"),
            Some(CtcpCommand::Unknown("FOO bar".to_string()))
        );
    }

    #[test]
    fn test_parse_ctcp_invalid() {
        assert_eq!(parse_ctcp("VERSION"), None);
        assert_eq!(parse_ctcp("\x01VERSION"), None);
        assert_eq!(parse_ctcp("VERSION\x01"), None);
        assert_eq!(parse_ctcp("\x01\x01"), None);
        assert_eq!(
            parse_ctcp("\x01 \x01"),
            Some(CtcpCommand::Unknown(" ".to_string()))
        );
    }

    #[test]
    fn test_parse_ctcp_ping_no_arg() {
        assert_eq!(
            parse_ctcp("\x01PING\x01"),
            Some(CtcpCommand::Ping("".to_string()))
        );
    }

    #[test]
    fn test_parse_ctcp_case_insensitive() {
        assert_eq!(parse_ctcp("\x01version\x01"), Some(CtcpCommand::Version));
        assert_eq!(
            parse_ctcp("\x01pInG abc\x01"),
            Some(CtcpCommand::Ping("abc".to_string()))
        );
    }
}
