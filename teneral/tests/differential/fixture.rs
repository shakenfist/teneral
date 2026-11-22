//! Decoding a captured frame from its hex-text fixture.
//!
//! A fixture is the bytes of one frame as they were seen on a wire,
//! written as two-digit hex bytes with `#` comments and any amount of
//! whitespace. Hex text rather than a binary blob for two reasons: a
//! reviewer can read the diff when a fixture changes, and the bytes
//! can carry annotations naming the fields they are. The annotations
//! are a reading aid -- the decoded bytes are what a case asserts on,
//! so a wrong comment cannot change a verdict.

/// The bytes a fixture holds.
pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let code = match line.split_once('#') {
            Some((before_comment, _)) => before_comment,
            None => line,
        };
        for token in code.split_whitespace() {
            bytes.push(byte(token, number)?);
        }
    }
    if bytes.is_empty() {
        return Err("holds no bytes at all, only comments and whitespace".to_owned());
    }
    Ok(bytes)
}

/// One hex byte, spelled exactly as two hex digits.
///
/// The two checks are not the same check. `from_str_radix` accepts a
/// sign, so `+1` would otherwise decode as `0x01`, and a fixture that
/// quietly accepts something that is not a byte is a fixture nobody
/// can trust to be the bytes that were captured.
fn byte(token: &str, line: usize) -> Result<u8, String> {
    let digits: Vec<char> = token.chars().collect();
    if digits.len() != 2 || !digits.iter().all(char::is_ascii_hexdigit) {
        return Err(format!(
            "line {line}: `{token}` is not a two-digit hex byte. A fixture is hex bytes, \
             whitespace and `#` comments, and nothing else."
        ));
    }
    u8::from_str_radix(token, 16).map_err(|why| format!("line {line}: `{token}`: {why}"))
}
