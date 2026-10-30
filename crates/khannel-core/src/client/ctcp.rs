#[derive(Debug, PartialEq)]
pub enum CtcpCommand {
    Version,
    Ping(String),
    Time,
    Unknown(String),
}

/// `\x01...\x01`로 감싼 CTCP 메시지면 안쪽 내용을 돌려준다 (비어 있으면 None).
pub fn ctcp_body(message: &str) -> Option<&str> {
    message
        .strip_prefix('\x01')?
        .strip_suffix('\x01')
        .filter(|inner| !inner.is_empty())
}

pub fn parse_ctcp(message: &str) -> Option<CtcpCommand> {
    let inner = ctcp_body(message)?;

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
        // 이전: 시작/끝 판정이 같은 \x01을 보고 범위가 뒤집혀 패닉
        assert_eq!(parse_ctcp("\x01"), None);
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
