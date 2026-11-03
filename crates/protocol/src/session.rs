//! Bounded shell-worker frames. OS paths and exported environment remain bytes.

use std::{
    ffi::{OsStr, OsString},
    future::poll_fn,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
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
///
/// [`Snapshot::new`] is the only place the wire invariants are checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
}

impl Snapshot {
    /// Build a snapshot from a raw cwd and the full exported environment;
    /// absent keys must not be inherited. Keys are sorted, because enumeration
    /// order is not part of the acquisition input.
    ///
    /// # Errors
    /// Rejects an empty or NUL cwd, and empty, duplicate, `=` or NUL keys and
    /// NUL values, without exposing their contents.
    pub fn new(cwd: PathBuf, mut env: Vec<(OsString, OsString)>) -> Result<Self, Error> {
        let raw = cwd.as_os_str().as_bytes();
        if raw.is_empty() || raw.contains(&0) {
            return Err(Error::Invalid("cwd"));
        }
        env.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let invalid = |(key, value): &(OsString, OsString)| {
            let key = key.as_bytes();
            key.is_empty()
                || key.contains(&0)
                || key.contains(&b'=')
                || value.as_bytes().contains(&0)
        };
        if env.iter().any(invalid) || env.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(Error::Invalid("environment"));
        }
        Ok(Self { cwd, env })
    }

    /// Raw current working directory.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Exported variables, sorted by key.
    #[must_use]
    pub fn vars(&self) -> &[(OsString, OsString)] {
        &self.env
    }

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
    /// Rejects invalid escaping and anything [`Snapshot::new`] rejects.
    pub fn decode(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() > MAX_REQUEST {
            return Err(Error::TooLarge(MAX_REQUEST));
        }
        let fields = frame.split(|byte| *byte == b'\t').collect::<Vec<_>>();
        let [
            b"Q",
            generation,
            exit,
            duration,
            cols,
            keymap,
            cwd,
            env @ ..,
        ] = fields.as_slice()
        else {
            return Err(Error::Invalid("request fields"));
        };
        let (env, []) = env.as_chunks::<2>() else {
            return Err(Error::Invalid("request fields"));
        };
        let env = env
            .iter()
            .map(|[key, value]| {
                Ok((
                    OsString::from_vec(unescape(key)?),
                    OsString::from_vec(unescape(value)?),
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self {
            generation: number(generation)?,
            last_exit_code: number(exit)?,
            duration_ms: if duration.is_empty() {
                None
            } else {
                Some(number(duration)?)
            },
            cols: number(cols)?,
            keymap: std::str::from_utf8(&unescape(keymap)?)?.to_owned(),
            snapshot: Snapshot::new(PathBuf::from(OsString::from_vec(unescape(cwd)?)), env)?,
        })
    }

    /// Encode a complete request including its LF delimiter.
    ///
    /// Every encoded request decodes to an equal value.
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
    use super::*;

    /// Covers: T06, T08
    #[test]
    fn invalid_frames_fail_without_exposing_values() {
        for frame in [
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA\tsecret\tA\tx".as_slice(),
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA\tsecret\0",
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA\tsecret\\q",
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA",
            b"Q\t1\t0\t\t80\tmain",
            b"R\t1\t0\t\t80\tmain\t/tmp",
            b"Q\t1\t0\t\t80\tmain\t",
            b"Q\t1\t0\t\t80\tmain\t/t\0p",
            b"Q\t1\t0\t\t80\tmain\t/tmp\t\tsecret",
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA\0\tsecret",
            b"Q\t1\t0\t\t80\tmain\t/tmp\tA=B\tsecret",
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

    /// Covers: T08
    #[test]
    fn size_limits_include_the_bound() -> Result<(), Error> {
        let mut request = b"Q\t1\t0\t\t80\tmain\t/".to_vec();
        request.resize(MAX_REQUEST, b'x');
        let encoded = Request::decode(&request)?.encode()?;
        assert_eq!(&encoded[..MAX_REQUEST], request.as_slice());
        request.push(b'x');
        assert!(matches!(Request::decode(&request), Err(Error::TooLarge(_))));

        let overhead = response(0, "", "", false)?.len() - 1;
        let line = "x".repeat(MAX_RESPONSE - overhead);
        let mut encoded = response(0, &line, "", false)?;
        encoded.pop();
        assert_eq!(Response::decode(&encoded)?.left1, line);
        assert!(response(0, &line, "x", false).is_err());
        encoded.push(b'x');
        assert!(matches!(
            Response::decode(&encoded),
            Err(Error::TooLarge(_))
        ));
        Ok(())
    }

    mod properties {
        use proptest::{collection::vec, prelude::*, sample::select};

        use super::*;

        const MARKER: &[u8] = b"s3cr3t-marker";

        /// Up to `max` values, biased toward `special` ones that the wire format treats specially.
        fn biased<T>(special: &'static [T], max: usize) -> impl Strategy<Value = Vec<T>>
        where
            T: Arbitrary + Clone + std::fmt::Debug + 'static,
        {
            vec(prop_oneof![3 => any::<T>(), 1 => select(special)], 0..max)
        }

        fn bytes(max: usize) -> impl Strategy<Value = Vec<u8>> {
            biased(b"\t\n\r\\=\0", max)
        }

        fn text(max: usize) -> impl Strategy<Value = String> {
            biased(&['\t', '\n', '\r', '\\'], max).prop_map(String::from_iter)
        }

        /// Any request whose snapshot satisfies the wire invariants.
        fn request() -> impl Strategy<Value = Request> {
            let fields = (
                any::<u64>(),
                any::<i32>(),
                any::<Option<u64>>(),
                any::<u16>(),
                text(6),
            );
            let parts = (bytes(8), vec((bytes(6), bytes(6)), 0..4));
            (fields, parts).prop_filter_map(
                "invalid snapshot",
                |((generation, last_exit_code, duration_ms, cols, keymap), (cwd, env))| {
                    let env = env
                        .into_iter()
                        .map(|(key, value)| (OsString::from_vec(key), OsString::from_vec(value)));
                    let cwd = PathBuf::from(OsString::from_vec(cwd));
                    Some(Request {
                        generation,
                        snapshot: Snapshot::new(cwd, env.collect()).ok()?,
                        cols,
                        last_exit_code,
                        duration_ms,
                        keymap,
                    })
                },
            )
        }

        proptest! {
            #[test]
            fn snapshots_round_trip_independent_of_environment_order(request in request()) {
                let snapshot = &request.snapshot;
                let reversed = snapshot.vars().iter().rev().cloned().collect();
                prop_assert_eq!(&Snapshot::new(snapshot.cwd().into(), reversed)?, snapshot);
                let frame = request.encode()?;
                let Some((b'\n', body)) = frame.split_last() else {
                    return Err(TestCaseError::fail("missing frame delimiter"));
                };
                prop_assert!(!body.contains(&b'\n'));
                prop_assert_eq!(Request::decode(body)?, request);
            }

            #[test]
            fn responses_round_trip(
                generation in any::<u64>(),
                left1 in text(16),
                left2 in text(16),
                complete in any::<bool>(),
            ) {
                let frame = response(generation, &left1, &left2, complete)?;
                let Some((b'\n', body)) = frame.split_last() else {
                    return Err(TestCaseError::fail("missing frame delimiter"));
                };
                prop_assert_eq!(
                    Response::decode(body)?,
                    Response { generation, left1, left2, complete }
                );
            }

            /// Covers: T06, T08
            #[test]
            fn rejected_frames_do_not_expose_environment_values(
                request in request(),
                index in any::<prop::sample::Index>(),
                byte in prop_oneof![any::<u8>(), select(b"\t\\\0=".as_slice())],
            ) {
                let secret = (OsString::from("SECRET"), OsString::from_vec(MARKER.to_vec()));
                let env = request.snapshot.vars().iter().cloned().chain([secret]).collect();
                let Ok(snapshot) = Snapshot::new(request.snapshot.cwd().into(), env) else {
                    return Ok(());
                };
                let mut frame = Request { snapshot, ..request }.encode()?;
                frame.pop();
                frame.insert(index.index(frame.len() + 1), byte);
                if let Err(error) = Request::decode(&frame) {
                    let message = error.to_string();
                    prop_assert!(
                        !message.as_bytes().windows(MARKER.len()).any(|window| window == MARKER),
                        "{message}"
                    );
                }
            }
        }
    }

    /// Coverage-guided targets; see docs/testing.md for fuzzing them.
    mod fuzz {
        use std::{
            collections::VecDeque,
            io,
            pin::Pin,
            task::{Context, Poll, Waker},
        };

        use tokio::io::ReadBuf;

        use super::*;

        const FRAME_LIMIT: usize = 8;

        /// Yields one chunk per read and reports `Pending` before each one.
        struct Chunked {
            chunks: VecDeque<Vec<u8>>,
            ready: bool,
        }

        impl AsyncRead for Chunked {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                self.ready = !self.ready;
                if !self.ready {
                    return Poll::Pending;
                }
                if let Some(mut chunk) = self.chunks.pop_front() {
                    let taken = chunk.len().min(buf.remaining());
                    buf.put_slice(&chunk[..taken]);
                    if taken < chunk.len() {
                        self.chunks.push_front(chunk.split_off(taken));
                    }
                }
                Poll::Ready(Ok(()))
            }
        }

        /// Covers: T08
        #[test]
        fn accepted_frames_re_encode_to_an_equal_value() {
            /// Re-decode an accepted frame's canonical encoding.
            fn canonical<T>(
                encoded: Result<Vec<u8>, Error>,
                decode: fn(&[u8]) -> Result<T, Error>,
            ) -> Option<T> {
                let encoded = encoded.ok()?;
                let canonical = &encoded[..encoded.len() - 1];
                assert!(!canonical.contains(&b'\n'));
                decode(canonical).ok()
            }
            bolero::check!().for_each(|frame: &[u8]| {
                if let Ok(request) = Request::decode(frame) {
                    assert!(
                        canonical(request.encode(), Request::decode)
                            .is_none_or(|decoded| decoded == request)
                    );
                }
                if let Ok(decoded) = Response::decode(frame) {
                    let Response {
                        generation,
                        ref left1,
                        ref left2,
                        complete,
                    } = decoded;
                    let encoded = response(generation, left1, left2, complete);
                    assert!(
                        canonical(encoded, Response::decode).is_none_or(|again| again == decoded)
                    );
                }
            });
        }

        #[derive(Debug, PartialEq, Eq)]
        enum End {
            Clean,
            FrameLimit,
            UnterminatedEof,
            Other(String),
        }

        /// Framing equals splitting the stream on LF up to the first frame over
        /// the limit, however reads are chunked and cancelled.
        ///
        /// Covers: T08
        #[test]
        fn framing_is_independent_of_chunking_and_cancellation() {
            bolero::check!()
                .with_type::<(Vec<u8>, Vec<u8>)>()
                .for_each(|(stream, sizes)| {
                    // Make one byte in eight a delimiter, so frames are short.
                    let stream = stream
                        .iter()
                        .map(|byte| if byte % 8 == 0 { b'\n' } else { *byte })
                        .collect::<Vec<_>>();
                    let mut pieces = stream.split(|byte| *byte == b'\n').collect::<Vec<_>>();
                    let tail = pieces.pop().unwrap_or_default();
                    let expected_frames = pieces
                        .iter()
                        .take_while(|frame| frame.len() <= FRAME_LIMIT)
                        .map(|frame| frame.to_vec())
                        .collect::<Vec<_>>();
                    let expected_end =
                        if expected_frames.len() < pieces.len() || tail.len() > FRAME_LIMIT {
                            End::FrameLimit
                        } else if tail.is_empty() {
                            End::Clean
                        } else {
                            End::UnterminatedEof
                        };

                    let mut chunks = VecDeque::new();
                    let mut rest = stream.as_slice();
                    for size in sizes.iter().chain([&1]).cycle() {
                        if rest.is_empty() {
                            break;
                        }
                        let size = usize::from(*size) % (2 * FRAME_LIMIT) + 1;
                        let (chunk, tail) = rest.split_at(size.min(rest.len()));
                        chunks.push_back(chunk.to_vec());
                        rest = tail;
                    }
                    let mut reader = FrameReader::new(
                        Chunked {
                            chunks,
                            ready: false,
                        },
                        FRAME_LIMIT,
                    );
                    let mut actual_frames = Vec::new();
                    let actual_end = loop {
                        // Each iteration drops the previous future, cancelling any partial read.
                        let mut next = std::pin::pin!(reader.next());
                        match next.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
                            Poll::Pending => {}
                            Poll::Ready(Ok(Some(frame))) => actual_frames.push(frame.to_vec()),
                            Poll::Ready(Ok(None)) => break End::Clean,
                            Poll::Ready(Err(Error::Invalid("frame limit"))) => {
                                break End::FrameLimit;
                            }
                            Poll::Ready(Err(Error::Io(error)))
                                if error.kind() == io::ErrorKind::UnexpectedEof =>
                            {
                                break End::UnterminatedEof;
                            }
                            Poll::Ready(Err(error)) => break End::Other(error.to_string()),
                        }
                    };
                    assert_eq!(actual_frames, expected_frames);
                    assert_eq!(actual_end, expected_end);
                });
        }
    }
}
