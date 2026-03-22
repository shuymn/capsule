//! Async codec for reading and writing messages over byte streams.
//!
//! [`MessageReader`] reads LF-delimited message lines from an [`AsyncRead`] source.
//! [`MessageWriter`] writes messages as LF-delimited lines to an [`AsyncWrite`] sink.

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::{ProtocolError, message::Message};

/// Reads messages from an async byte stream.
///
/// Wraps the underlying reader in a [`BufReader`] and reads one LF-delimited
/// message per call to [`read_message`](Self::read_message).
pub struct MessageReader<R> {
    inner: BufReader<R>,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin + Send> MessageReader<R> {
    /// Create a new reader wrapping the given byte stream.
    #[must_use]
    pub fn new(reader: R) -> Self {
        Self {
            inner: BufReader::new(reader),
            buf: Vec::with_capacity(512),
        }
    }

    /// Read the next message from the stream.
    ///
    /// Returns `Ok(None)` on EOF (clean session end).
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] on I/O failure or if the message cannot be parsed.
    pub async fn read_message(&mut self) -> Result<Option<Message>, ProtocolError> {
        self.buf.clear();
        let n = self.inner.read_until(b'\n', &mut self.buf).await?;
        if n == 0 {
            return Ok(None);
        }
        // Strip trailing LF
        if self.buf.last() == Some(&b'\n') {
            self.buf.pop();
        }
        Message::from_wire(&self.buf).map(Some)
    }
}

/// Writes messages to an async byte stream.
///
/// Each message is written as its wire representation followed by `\n`,
/// then flushed.
pub struct MessageWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin + Send> MessageWriter<W> {
    /// Create a new writer wrapping the given byte stream.
    #[must_use]
    pub const fn new(writer: W) -> Self {
        Self { inner: writer }
    }

    /// Write a message to the stream, followed by `\n`, then flush.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::Io`] on write failure.
    pub async fn write_message(&mut self, msg: &Message) -> Result<(), ProtocolError> {
        let wire = msg.to_wire();
        self.inner.write_all(&wire).await?;
        self.inner.write_all(b"\n").await?;
        self.inner.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PromptGeneration,
        message::{Hello, HelloAck, RenderResult, Request, SessionId, Update},
    };

    fn sample_session_id() -> SessionId {
        SessionId::from_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
    }

    fn sample_request() -> Request {
        Request {
            version: 1,
            session_id: sample_session_id(),
            generation: PromptGeneration::new(1),
            cwd: "/tmp".to_owned(),
            cols: 80,
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main".to_owned(),
            env_vars: vec![],
        }
    }

    fn sample_render_result() -> RenderResult {
        RenderResult {
            version: 1,
            session_id: sample_session_id(),
            generation: PromptGeneration::new(1),
            left1: "/tmp".to_owned(),
            left2: "❯ ".to_owned(),
            meta: String::new(),
        }
    }

    fn sample_update() -> Update {
        Update {
            version: 1,
            session_id: sample_session_id(),
            generation: PromptGeneration::new(1),
            left1: "/tmp  main".to_owned(),
            left2: "❯ ".to_owned(),
            meta: String::new(),
        }
    }

    #[tokio::test]
    async fn test_codec_round_trip_request() -> Result<(), ProtocolError> {
        let (client, server) = tokio::io::duplex(4096);
        let mut writer = MessageWriter::new(client);
        let mut reader = MessageReader::new(server);

        let msg = Message::Request(sample_request());
        writer.write_message(&msg).await?;
        drop(writer); // signal EOF after the message

        let received = reader.read_message().await?;
        assert_eq!(received, Some(msg));

        let eof = reader.read_message().await?;
        assert_eq!(eof, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_codec_round_trip_render_result() -> Result<(), ProtocolError> {
        let (client, server) = tokio::io::duplex(4096);
        let mut writer = MessageWriter::new(client);
        let mut reader = MessageReader::new(server);

        let msg = Message::RenderResult(sample_render_result());
        writer.write_message(&msg).await?;
        drop(writer);

        let received = reader.read_message().await?;
        assert_eq!(received, Some(msg));
        Ok(())
    }

    #[tokio::test]
    async fn test_codec_round_trip_update() -> Result<(), ProtocolError> {
        let (client, server) = tokio::io::duplex(4096);
        let mut writer = MessageWriter::new(client);
        let mut reader = MessageReader::new(server);

        let msg = Message::Update(sample_update());
        writer.write_message(&msg).await?;
        drop(writer);

        let received = reader.read_message().await?;
        assert_eq!(received, Some(msg));
        Ok(())
    }

    #[tokio::test]
    async fn test_codec_multiple_messages() -> Result<(), ProtocolError> {
        let (client, server) = tokio::io::duplex(4096);
        let mut writer = MessageWriter::new(client);
        let mut reader = MessageReader::new(server);

        let msg1 = Message::Request(sample_request());
        let msg2 = Message::RenderResult(sample_render_result());
        let msg3 = Message::Update(sample_update());

        writer.write_message(&msg1).await?;
        writer.write_message(&msg2).await?;
        writer.write_message(&msg3).await?;
        drop(writer);

        assert_eq!(reader.read_message().await?, Some(msg1));
        assert_eq!(reader.read_message().await?, Some(msg2));
        assert_eq!(reader.read_message().await?, Some(msg3));
        assert_eq!(reader.read_message().await?, None);
        Ok(())
    }

    fn sample_hello() -> Hello {
        Hello {
            version: 1,
            build_id: Some(crate::BuildId::new("12345:1700000000000000000".to_owned())),
        }
    }

    fn sample_hello_ack() -> HelloAck {
        HelloAck {
            version: 1,
            build_id: Some(crate::BuildId::new("12345:1700000000000000000".to_owned())),
            env_var_names: vec![],
        }
    }

    #[tokio::test]
    async fn test_codec_round_trip_hello() -> Result<(), ProtocolError> {
        let (client, server) = tokio::io::duplex(4096);
        let mut writer = MessageWriter::new(client);
        let mut reader = MessageReader::new(server);

        let msg = Message::Hello(sample_hello());
        writer.write_message(&msg).await?;
        drop(writer);

        let received = reader.read_message().await?;
        assert_eq!(received, Some(msg));
        Ok(())
    }

    #[tokio::test]
    async fn test_codec_round_trip_hello_ack() -> Result<(), ProtocolError> {
        let (client, server) = tokio::io::duplex(4096);
        let mut writer = MessageWriter::new(client);
        let mut reader = MessageReader::new(server);

        let msg = Message::HelloAck(sample_hello_ack());
        writer.write_message(&msg).await?;
        drop(writer);

        let received = reader.read_message().await?;
        assert_eq!(received, Some(msg));
        Ok(())
    }

    #[tokio::test]
    async fn test_codec_eof_on_empty_stream() -> Result<(), ProtocolError> {
        let (client, server) = tokio::io::duplex(4096);
        drop(client); // immediate EOF

        let mut reader = MessageReader::new(server);
        let result = reader.read_message().await?;
        assert_eq!(result, None);
        Ok(())
    }
}
