//! Bounded shell-worker frames. OS paths and exported environment remain bytes.

use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    future::poll_fn,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    pin::Pin,
};

use futures_core::Stream;
use tokio::io::AsyncRead;
use tokio_util::{
    bytes::{Bytes, BytesMut},
    codec::{AnyDelimiterCodec, AnyDelimiterCodecError, Decoder, FramedRead},
};

/// Maximum escaped request size, excluding its LF delimiter.
pub const MAX_REQUEST: usize = 256 * 1024;
/// Maximum escaped response size, excluding its LF delimiter.
pub const MAX_RESPONSE: usize = 64 * 1024;

/// Complete acquisition input for one shell execution generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Raw current working directory.
    pub cwd: PathBuf,
    /// Full exported environment; absent keys must not be inherited.
    pub env: Vec<(OsString, OsString)>,
}

impl Snapshot {
    /// Look up a variable without converting its OS bytes.
    #[must_use]
    pub fn env(&self, name: &str) -> Option<&OsStr> {
        self.env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_os_str())
    }
}

/// Complete render request; equal generations reuse acquisition observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Execution generation chosen by the shell.
    pub generation: u64,
    /// Captured cwd and environment.
    pub snapshot: Snapshot,
    /// Available terminal columns.
    pub cols: u16,
    /// Most recent command exit status.
    pub last_exit_code: i32,
    /// Most recent command duration.
    pub duration_ms: Option<u64>,
    /// Current zle keymap.
    pub keymap: String,
}

/// One render, including whether all generation acquisitions have settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// Matching execution generation.
    pub generation: u64,
    /// Serialized information line.
    pub left1: String,
    /// Serialized input line.
    pub left2: String,
    /// True after all acquisition tasks and their cleanup have settled.
    pub complete: bool,
}

impl Response {
    /// Decode a response; credit frames are handled separately by the transport.
    ///
    /// # Errors
    /// Rejects malformed, oversized or non-UTF-8 responses.
    pub fn decode(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() > MAX_RESPONSE {
            return Err(Error::TooLarge(MAX_RESPONSE));
        }
        let fields = frame.split(|byte| *byte == b'\t').collect::<Vec<_>>();
        let [b"R", generation, left1, left2, complete] = fields.as_slice() else {
            return Err(Error::Invalid("response fields"));
        };
        Ok(Self {
            generation: number(generation)?,
            left1: std::str::from_utf8(&unescape(left1)?)?.to_owned(),
            left2: std::str::from_utf8(&unescape(left2)?)?.to_owned(),
            complete: match *complete {
                b"0" => false,
                b"1" => true,
                _ => return Err(Error::Invalid("completion")),
            },
        })
    }
}

/// Invalid or oversized session data. Errors never include environment contents.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid number, shape, escaping, or OS field.
    #[error("invalid shell frame: {0}")]
    Invalid(&'static str),
    /// Bounded input or output exceeded its limit.
    #[error("shell frame exceeds {0} bytes")]
    TooLarge(usize),
    /// Invalid numeric field; the error contains no original input.
    #[error("invalid shell number: {0}")]
    Number(#[from] std::num::ParseIntError),
    /// A textual control field contains invalid UTF-8.
    #[error("invalid shell control text: {0}")]
    Text(#[from] std::str::Utf8Error),
    /// Failed pipe operation.
    #[error("shell pipe: {0}")]
    Io(#[from] std::io::Error),
}

/// Persistent cancellation-safe byte framing, including non-UTF-8 payloads.
pub struct FrameReader<R>(FramedRead<R, CompleteLines>);

struct CompleteLines(AnyDelimiterCodec);

impl Decoder for CompleteLines {
    type Item = Bytes;
    type Error = AnyDelimiterCodecError;

    fn decode(&mut self, input: &mut BytesMut) -> Result<Option<Bytes>, Self::Error> {
        self.0.decode(input)
    }

    fn decode_eof(&mut self, input: &mut BytesMut) -> Result<Option<Bytes>, Self::Error> {
        let frame = self.decode(input)?;
        if frame.is_some() || input.is_empty() {
            return Ok(frame);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "unterminated shell frame",
        )
        .into())
    }
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// Wrap a pipe with a maximum frame length.
    #[must_use]
    pub fn new(input: R, limit: usize) -> Self {
        Self(FramedRead::new(
            input,
            CompleteLines(AnyDelimiterCodec::new_with_max_length(
                vec![b'\n'],
                vec![b'\n'],
                limit,
            )),
        ))
    }

    /// Read one frame, retaining partial bytes across cancellation.
    ///
    /// # Errors
    /// Returns an error for oversized frames or I/O failure.
    pub async fn next(&mut self) -> Result<Option<tokio_util::bytes::Bytes>, Error> {
        poll_fn(|cx| Pin::new(&mut self.0).poll_next(cx))
            .await
            .transpose()
            .map_err(|error| match error {
                AnyDelimiterCodecError::MaxChunkLengthExceeded => Error::Invalid("frame limit"),
                AnyDelimiterCodecError::Io(error) => Error::Io(error),
            })
    }
}

impl Request {
    /// Decode `Q, generation, exit, duration, cols, keymap, cwd, (key,value)*`.
    ///
    /// # Errors
    /// Rejects invalid escaping, duplicate/invalid environment keys and NUL bytes.
    pub fn decode(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() > MAX_REQUEST {
            return Err(Error::TooLarge(MAX_REQUEST));
        }
        let fields = frame.split(|byte| *byte == b'\t').collect::<Vec<_>>();
        if fields.len() < 7 || fields[0] != b"Q" || (fields.len() - 7) % 2 != 0 {
            return Err(Error::Invalid("request fields"));
        }
        let cwd = unescape(fields[6])?;
        if cwd.is_empty() || cwd.contains(&0) {
            return Err(Error::Invalid("cwd"));
        }
        let mut env = Vec::with_capacity((fields.len() - 7) / 2);
        let mut keys = HashSet::new();
        for [raw_key, raw_value] in fields[7..].as_chunks::<2>().0 {
            let key = unescape(raw_key)?;
            let value = unescape(raw_value)?;
            if key.is_empty()
                || key.contains(&0)
                || key.contains(&b'=')
                || value.contains(&0)
                || !keys.insert(key.clone())
            {
                return Err(Error::Invalid("environment"));
            }
            env.push((OsString::from_vec(key), OsString::from_vec(value)));
        }
        Ok(Self {
            generation: number(fields[1])?,
            last_exit_code: number(fields[2])?,
            duration_ms: if fields[3].is_empty() {
                None
            } else {
                Some(number(fields[3])?)
            },
            cols: number(fields[4])?,
            keymap: std::str::from_utf8(&unescape(fields[5])?)?.to_owned(),
            snapshot: Snapshot {
                cwd: PathBuf::from(OsString::from_vec(cwd)),
                env,
            },
        })
    }

    /// Encode a complete request including its LF delimiter.
    ///
    /// # Errors
    /// Returns an error if the escaped request exceeds the bound.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut output = format!(
            "Q\t{}\t{}\t{}\t{}\t",
            self.generation,
            self.last_exit_code,
            self.duration_ms
                .map_or_else(String::new, |value| value.to_string()),
            self.cols
        )
        .into_bytes();
        escape_into(self.keymap.as_bytes(), &mut output);
        output.push(b'\t');
        escape_into(self.snapshot.cwd.as_os_str().as_bytes(), &mut output);
        for (key, value) in &self.snapshot.env {
            output.push(b'\t');
            escape_into(key.as_bytes(), &mut output);
            output.push(b'\t');
            escape_into(value.as_bytes(), &mut output);
        }
        finish(output, MAX_REQUEST)
    }
}

/// Encode already serialized prompt lines, preserving frame boundaries.
///
/// # Errors
/// Returns an error if the response exceeds the bound.
pub fn response(
    generation: u64,
    left1: &str,
    left2: &str,
    complete: bool,
) -> Result<Vec<u8>, Error> {
    let mut output = format!("R\t{generation}\t").into_bytes();
    escape_into(left1.as_bytes(), &mut output);
    output.push(b'\t');
    escape_into(left2.as_bytes(), &mut output);
    output.extend_from_slice(if complete { b"\t1" } else { b"\t0" });
    finish(output, MAX_RESPONSE)
}

fn finish(mut output: Vec<u8>, limit: usize) -> Result<Vec<u8>, Error> {
    if output.len() > limit {
        return Err(Error::TooLarge(limit));
    }
    output.push(b'\n');
    Ok(output)
}

fn number<T: std::str::FromStr<Err = std::num::ParseIntError>>(bytes: &[u8]) -> Result<T, Error> {
    Ok(std::str::from_utf8(bytes)?.parse()?)
}

fn escape_into(input: &[u8], output: &mut Vec<u8>) {
    for byte in input {
        match byte {
            b'\\' => output.extend_from_slice(b"\\\\"),
            b'\t' => output.extend_from_slice(b"\\t"),
            b'\n' => output.extend_from_slice(b"\\n"),
            b'\r' => output.extend_from_slice(b"\\r"),
            byte => output.push(*byte),
        }
    }
}

fn unescape(input: &[u8]) -> Result<Vec<u8>, Error> {
    let mut result = Vec::with_capacity(input.len());
    let mut bytes = input.iter().copied();
    while let Some(byte) = bytes.next() {
        result.push(if byte == b'\\' {
            match bytes.next() {
                Some(b'\\') => b'\\',
                Some(b'n') => b'\n',
                Some(b'r') => b'\r',
                Some(b't') => b'\t',
                _ => return Err(Error::Invalid("escape")),
            }
        } else {
            byte
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        task::{Context, Waker},
    };

    use tokio::io::AsyncWriteExt;

    use super::*;

    #[tokio::test]
    async fn cancellation_preserves_chunks_and_following_frames() -> Result<(), Error> {
        let (mut input, output) = tokio::io::duplex(4096);
        let mut reader = FrameReader::new(output, 1024);
        let frame = b"Q\t3\t0\t\t80\tmain\t/tmp/\xff\tEMPTY\t";
        for chunk in frame.chunks(7) {
            input.write_all(chunk).await?;
            let mut read = std::pin::pin!(reader.next());
            assert!(
                read.as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        input.write_all(b"\nK\n").await?;
        drop(input);
        assert_eq!(reader.next().await?.as_deref(), Some(frame.as_slice()));
        assert_eq!(reader.next().await?.as_deref(), Some(b"K".as_slice()));
        assert_eq!(reader.next().await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn frame_bounds_and_truncated_eof_are_enforced() -> Result<(), Error> {
        for length in [63, 64, 65] {
            let mut frame = vec![b'x'; length];
            frame.push(b'\n');
            let result = FrameReader::new(frame.as_slice(), 64).next().await;
            assert_eq!(result.is_ok(), length <= 64);
        }
        let mut reader = FrameReader::new(b"K\nQ\t1\t0\t\t80\tmain\t/tmp".as_slice(), MAX_REQUEST);
        assert_eq!(reader.next().await?.as_deref(), Some(b"K".as_slice()));
        assert!(
            matches!(reader.next().await, Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof)
        );
        Ok(())
    }

    #[test]
    fn raw_environment_roundtrip_distinguishes_empty_from_absent() -> Result<(), Error> {
        let request =
            Request::decode(b"Q\t3\t0\t\t80\tmain\t/tmp/\xff\tEMPTY\t\tRAW\t\xff\\n\\t\\\\")?;
        assert_eq!(request.snapshot.env("EMPTY"), Some(OsStr::new("")));
        assert_eq!(request.snapshot.env("ABSENT"), None);
        assert_eq!(
            request.snapshot.env("RAW").map(OsStrExt::as_bytes),
            Some(&b"\xff\n\t\\"[..])
        );
        let encoded = request.encode()?;
        assert_eq!(Request::decode(&encoded[..encoded.len() - 1])?, request);
        Ok(())
    }

    #[test]
    fn invalid_frames_fail_without_exposing_values() {
        for frame in [
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA\tsecret\tA\tx".as_slice(),
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA\tsecret\0",
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA\tsecret\\q",
        ] {
            let result = Request::decode(frame);
            assert!(result.is_err());
            assert!(
                !result
                    .err()
                    .is_some_and(|error| error.to_string().contains("secret"))
            );
        }
    }
}
