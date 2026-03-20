//! Netstring encoding and decoding.
//!
//! A netstring is `<length>:<data>,` where `<length>` is the decimal byte count of `<data>`.

use crate::ProtocolError;

/// Encode `data` as a netstring: `<len>:<data>,`.
///
/// # Examples
///
/// ```
/// # use capsule_protocol::netstring;
/// assert_eq!(netstring::encode(b"hello"), b"5:hello,");
/// assert_eq!(netstring::encode(b""), b"0:,");
/// ```
/// Append `data` as a netstring directly into `buf`, avoiding intermediate allocation.
pub fn encode_into(buf: &mut Vec<u8>, data: &[u8]) {
    use std::io::Write as _;
    // write! to Vec<u8> via std::io::Write is infallible
    let _ = write!(buf, "{}:", data.len());
    buf.extend_from_slice(data);
    buf.push(b',');
}

/// Encode `data` as a netstring: `<len>:<data>,`.
///
/// # Examples
///
/// ```
/// # use capsule_protocol::netstring;
/// assert_eq!(netstring::encode(b"hello"), b"5:hello,");
/// assert_eq!(netstring::encode(b""), b"0:,");
/// ```
#[must_use]
pub fn encode(data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(data.len() + 10);
    encode_into(&mut buf, data);
    buf
}

/// Decode one netstring from `input`.
///
/// Returns `(data, remaining_input)` on success.
///
/// # Errors
///
/// Returns [`ProtocolError`] if the input is not a valid netstring:
/// - [`InvalidLength`](ProtocolError::InvalidLength) if the length prefix is not a valid number
/// - [`MissingColon`](ProtocolError::MissingColon) if no `:` separator is found
/// - [`MissingComma`](ProtocolError::MissingComma) if the trailing `,` is missing
/// - [`Truncated`](ProtocolError::Truncated) if the input ends before the data is complete
pub fn decode(input: &[u8]) -> Result<(&[u8], &[u8]), ProtocolError> {
    let colon_pos = input
        .iter()
        .position(|&b| b == b':')
        .ok_or(ProtocolError::MissingColon)?;

    let len_str =
        std::str::from_utf8(&input[..colon_pos]).map_err(|_e| ProtocolError::InvalidLength)?;
    let len: usize = len_str.parse().map_err(|_e| ProtocolError::InvalidLength)?;

    let data_start = colon_pos + 1;
    let data_end = data_start
        .checked_add(len)
        .ok_or(ProtocolError::InvalidLength)?;

    // Need data_end index for the trailing comma
    if data_end >= input.len() {
        return Err(ProtocolError::Truncated);
    }

    if input[data_end] != b',' {
        return Err(ProtocolError::MissingComma);
    }

    Ok((&input[data_start..data_end], &input[data_end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_empty() {
        assert_eq!(encode(b""), b"0:,");
    }

    #[test]
    fn test_encode_ascii() {
        assert_eq!(encode(b"hello"), b"5:hello,");
    }

    #[test]
    fn test_encode_utf8() {
        let data = "こんにちは".as_bytes();
        let encoded = encode(data);
        assert_eq!(
            encoded,
            format!("{}:{},", data.len(), "こんにちは").into_bytes()
        );
    }

    #[test]
    fn test_encode_long() {
        let data = vec![0x42u8; 1000];
        let encoded = encode(&data);
        assert_eq!(&encoded[..5], b"1000:");
        assert_eq!(encoded[encoded.len() - 1], b',');
        assert_eq!(encoded.len(), 4 + 1 + 1000 + 1); // "1000" + ":" + data + ","
    }

    #[test]
    fn test_round_trip_empty() -> Result<(), ProtocolError> {
        let encoded = encode(b"");
        let (data, rest) = decode(&encoded)?;
        assert_eq!(data, b"");
        assert!(rest.is_empty());
        Ok(())
    }

    #[test]
    fn test_round_trip_ascii() -> Result<(), ProtocolError> {
        let encoded = encode(b"hello world");
        let (data, rest) = decode(&encoded)?;
        assert_eq!(data, b"hello world");
        assert!(rest.is_empty());
        Ok(())
    }

    #[test]
    fn test_round_trip_utf8() -> Result<(), ProtocolError> {
        let original = "日本語テスト".as_bytes();
        let encoded = encode(original);
        let (data, rest) = decode(&encoded)?;
        assert_eq!(data, original);
        assert!(rest.is_empty());
        Ok(())
    }

    #[test]
    fn test_round_trip_long() -> Result<(), ProtocolError> {
        let original = vec![0xFFu8; 10_000];
        let encoded = encode(&original);
        let (data, rest) = decode(&encoded)?;
        assert_eq!(data, &original[..]);
        assert!(rest.is_empty());
        Ok(())
    }

    #[test]
    fn test_decode_multiple() -> Result<(), ProtocolError> {
        let mut wire = encode(b"aaa");
        wire.extend_from_slice(&encode(b"bb"));
        wire.extend_from_slice(&encode(b""));

        let (first, rest) = decode(&wire)?;
        assert_eq!(first, b"aaa");
        let (second, rest) = decode(rest)?;
        assert_eq!(second, b"bb");
        let (third, rest) = decode(rest)?;
        assert_eq!(third, b"");
        assert!(rest.is_empty());
        Ok(())
    }

    #[test]
    fn test_decode_error_missing_colon() {
        let result = decode(b"5hello,");
        assert!(matches!(result, Err(ProtocolError::MissingColon)));
    }

    #[test]
    fn test_decode_error_invalid_length() {
        let result = decode(b"abc:hello,");
        assert!(matches!(result, Err(ProtocolError::InvalidLength)));
    }

    #[test]
    fn test_decode_error_missing_comma() {
        let result = decode(b"5:hello.");
        assert!(matches!(result, Err(ProtocolError::MissingComma)));
    }

    #[test]
    fn test_decode_error_truncated() {
        let result = decode(b"10:hello,");
        assert!(matches!(result, Err(ProtocolError::Truncated)));
    }

    #[test]
    fn test_decode_error_empty_input() {
        let result = decode(b"");
        assert!(matches!(result, Err(ProtocolError::MissingColon)));
    }

    #[test]
    fn test_data_containing_colon() -> Result<(), ProtocolError> {
        let original = b"key:value";
        let encoded = encode(original);
        let (data, rest) = decode(&encoded)?;
        assert_eq!(data, original);
        assert!(rest.is_empty());
        Ok(())
    }

    #[test]
    fn test_data_containing_comma() -> Result<(), ProtocolError> {
        let original = b"a,b,c";
        let encoded = encode(original);
        let (data, rest) = decode(&encoded)?;
        assert_eq!(data, original);
        assert!(rest.is_empty());
        Ok(())
    }
}
