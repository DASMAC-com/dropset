// cspell:word AGENTC
// cspell:word EINVAL
//! A [`Signer`] backed by an SSH agent — the 1Password SSH agent in
//! practice — so a Solana private key can sign without ever leaving the
//! agent's process.
//!
//! The premise: an SSH agent signs whatever bytes a client hands it with
//! the named key, and for an Ed25519 key that is a plain Ed25519 signature
//! over the bytes (RFC 8709 section 6). A Solana transaction signature is a
//! plain Ed25519 signature over the serialized message, so the agent's
//! answer *is* the transaction signature. Every request is checked by
//! verifying the returned signature against the key's address before it is
//! handed back, so a confused agent (or a keypair whose seed does not derive
//! its public half) fails loudly instead of producing a transaction the
//! cluster rejects.
//!
//! The client speaks only the two requests it needs of the agent protocol
//! (draft-miller-ssh-agent: request-identities and sign-request), their
//! answers and the failure reply, over the Unix socket, one connection per
//! request. Discovery follows the order SSH itself would use, then
//! 1Password's own paths (see [`candidate_sockets`]); an explicit socket
//! overrides all of them. A socket listing a key is not proof it holds it:
//! one that lies wins discovery and then fails every signature, which
//! verification turns into an error, never a bad signature.
//!
//! Not yet wired into the cockpit.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::{Signer, SignerError};

/// `SSH_AGENT_FAILURE`.
const AGENT_FAILURE: u8 = 5;
/// `SSH_AGENTC_REQUEST_IDENTITIES`.
const AGENTC_REQUEST_IDENTITIES: u8 = 11;
/// `SSH_AGENT_IDENTITIES_ANSWER`.
const AGENT_IDENTITIES_ANSWER: u8 = 12;
/// `SSH_AGENTC_SIGN_REQUEST`.
const AGENTC_SIGN_REQUEST: u8 = 13;
/// `SSH_AGENT_SIGN_RESPONSE`.
const AGENT_SIGN_RESPONSE: u8 = 14;
/// The largest reply accepted; OpenSSH's own agent caps messages at 256 KiB.
const MAX_MESSAGE: usize = 256 * 1024;
const ED25519: &[u8] = b"ssh-ed25519";

/// How long an identity listing's reply may take, in total. It never prompts.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a sign request's reply may take, in total — long enough for the
/// operator to read the approval prompt and answer it.
const SIGN_TIMEOUT: Duration = Duration::from_secs(120);
/// How long writing any request may take; the agent reads before it prompts.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// 1Password's team id, which prefixes its macOS group-container path.
const OP_GROUP_CONTAINER: &str = "2BUA8C4S2C.com.1password";

/// One key the agent serves: its wire-format public-key blob and comment
/// (1Password sets the comment to the item's title).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub blob: Vec<u8>,
    pub comment: String,
}

impl Identity {
    /// The key type named at the head of the blob (`ssh-ed25519`, `ssh-rsa`).
    pub fn key_type(&self) -> String {
        Reader::new(&self.blob)
            .string()
            .map(|t| String::from_utf8_lossy(t).into_owned())
            .unwrap_or_else(|_| "<malformed>".to_string())
    }

    /// The Solana address of an Ed25519 identity; `None` for any other type.
    pub fn ed25519_pubkey(&self) -> Option<Pubkey> {
        let mut r = Reader::new(&self.blob);
        if r.string().ok()? != ED25519 {
            return None;
        }
        let key: [u8; 32] = r.string().ok()?.try_into().ok()?;
        r.is_empty().then_some(Pubkey::new_from_array(key))
    }
}

/// The wire-format public-key blob for an Ed25519 address.
fn ed25519_blob(pubkey: &Pubkey) -> Vec<u8> {
    let mut blob = Vec::with_capacity(4 + ED25519.len() + 4 + 32);
    put_string(&mut blob, ED25519);
    put_string(&mut blob, pubkey.as_ref());
    blob
}

/// The agent sockets to try, in order: `$SSH_AUTH_SOCK` (what SSH uses),
/// then `~/.1password/agent.sock` (the symlink 1Password's docs set up),
/// then the socket 1Password itself creates in its macOS group container.
/// Unset or empty entries are skipped and duplicates kept out; whether a
/// socket exists is left to the connect.
pub fn candidate_sockets(ssh_auth_sock: Option<OsString>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: PathBuf| {
        if !p.as_os_str().is_empty() && !out.contains(&p) {
            out.push(p);
        }
    };
    if let Some(sock) = ssh_auth_sock {
        push(PathBuf::from(sock));
    }
    if let Some(home) = home {
        push(home.join(".1password/agent.sock"));
        push(
            home.join("Library/Group Containers")
                .join(OP_GROUP_CONTAINER)
                .join("t/agent.sock"),
        );
    }
    out
}

/// The candidate sockets for this process's environment.
pub fn default_candidate_sockets() -> Vec<PathBuf> {
    candidate_sockets(
        std::env::var_os("SSH_AUTH_SOCK"),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// Every identity the agent at `socket` serves.
pub fn list_identities(socket: &Path) -> Result<Vec<Identity>> {
    let reply = request(socket, &[AGENTC_REQUEST_IDENTITIES], LIST_TIMEOUT)?;
    let mut r = Reader::new(&reply);
    match r.byte()? {
        AGENT_IDENTITIES_ANSWER => {}
        AGENT_FAILURE => bail!("agent refused to list identities"),
        t => bail!("unexpected agent reply type {t} to an identity listing"),
    }
    let n = r.u32()?;
    let mut ids = Vec::new();
    for _ in 0..n {
        let blob = r.string()?.to_vec();
        let comment = String::from_utf8_lossy(r.string()?).into_owned();
        ids.push(Identity { blob, comment });
    }
    Ok(ids)
}

/// A [`Signer`] for one Ed25519 address, signing through an SSH agent.
#[derive(Clone, Debug)]
pub struct AgentSigner {
    socket: PathBuf,
    pubkey: Pubkey,
}

impl AgentSigner {
    /// Find an agent serving `pubkey`. With `socket` set only that socket is
    /// tried; otherwise each of [`default_candidate_sockets`] is, in order, and
    /// the first that serves the key wins — so a system agent at
    /// `$SSH_AUTH_SOCK` that lacks it falls through to 1Password's.
    pub fn connect(pubkey: Pubkey, socket: Option<&Path>) -> Result<Self> {
        let candidates = match socket {
            Some(s) => vec![s.to_path_buf()],
            None => default_candidate_sockets(),
        };
        Self::connect_among(pubkey, &candidates)
    }

    /// [`connect`](Self::connect) over an explicit candidate list.
    pub fn connect_among(pubkey: Pubkey, candidates: &[PathBuf]) -> Result<Self> {
        let mut tried = Vec::new();
        for socket in candidates {
            match list_identities(socket) {
                Ok(ids) if ids.iter().any(|id| id.ed25519_pubkey() == Some(pubkey)) => {
                    return Ok(Self {
                        socket: socket.clone(),
                        pubkey,
                    });
                }
                Ok(ids) => tried.push(format!(
                    "{}: {} identities, none is {pubkey}",
                    socket.display(),
                    ids.len()
                )),
                Err(e) => tried.push(format!("{}: {e:#}", socket.display())),
            }
        }
        if tried.is_empty() {
            bail!("no SSH agent socket to try");
        }
        bail!("no SSH agent serves {pubkey}:\n  {}", tried.join("\n  "))
    }

    /// The socket this signer talks to.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        let mut req = vec![AGENTC_SIGN_REQUEST];
        put_string(&mut req, &ed25519_blob(&self.pubkey));
        put_string(&mut req, message);
        req.extend_from_slice(&0u32.to_be_bytes()); // flags: none apply to Ed25519
        let reply = request(&self.socket, &req, SIGN_TIMEOUT).map_err(|e| match e {
            RequestError::Connect(m) => SignerError::Connection(m),
            // How 1Password answers a declined prompt (a failure reply or a
            // closed socket) is unmeasured, so both read as a cancel.
            RequestError::NoAnswer(m) => {
                SignerError::UserCancel(format!("{m} (prompt declined or not answered?)"))
            }
            RequestError::Frame(m) => SignerError::Protocol(m),
        })?;
        let protocol = |e: anyhow::Error| SignerError::Protocol(format!("{e:#}"));
        let mut r = Reader::new(&reply);
        match r.byte().map_err(protocol)? {
            AGENT_SIGN_RESPONSE => {}
            AGENT_FAILURE => {
                return Err(SignerError::UserCancel(
                    "the agent refused to sign (prompt declined, or the key is no longer served)"
                        .to_string(),
                ))
            }
            t => {
                return Err(SignerError::Protocol(format!(
                    "unexpected agent reply type {t}"
                )))
            }
        }
        let mut sig = Reader::new(r.string().map_err(protocol)?);
        if sig.string().map_err(protocol)? != ED25519 {
            return Err(SignerError::Protocol(
                "agent signed with a non-Ed25519 key".into(),
            ));
        }
        let bytes: [u8; 64] = sig
            .string()
            .map_err(protocol)?
            .try_into()
            .map_err(|_| SignerError::Protocol("Ed25519 signature is not 64 bytes".into()))?;
        let signature = Signature::from(bytes);
        if !signature.verify(self.pubkey.as_ref(), message) {
            return Err(SignerError::Protocol(format!(
                "the agent's signature does not verify against {}",
                self.pubkey
            )));
        }
        Ok(signature)
    }
}

impl Signer for AgentSigner {
    fn try_pubkey(&self) -> Result<Pubkey, SignerError> {
        Ok(self.pubkey)
    }

    fn try_sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
        self.sign(message)
    }

    /// Every signature may prompt the operator.
    fn is_interactive(&self) -> bool {
        true
    }
}

/// Why one agent round trip failed, kept apart so a signer can tell a dead
/// socket from an unanswered request from a malformed reply.
#[derive(Debug)]
enum RequestError {
    /// Connecting or writing failed: no agent there, or it went away.
    Connect(String),
    /// The agent hung up, or ran out the deadline, before replying.
    NoAnswer(String),
    /// The reply was not a well-formed frame.
    Frame(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Connect(m) | Self::NoAnswer(m) | Self::Frame(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for RequestError {}

/// Send one framed request and read one framed reply within `timeout`.
fn request(socket: &Path, payload: &[u8], timeout: Duration) -> Result<Vec<u8>, RequestError> {
    let at = socket.display();
    let connect = |what: &str, e: io::Error| RequestError::Connect(format!("{what} {at}: {e}"));
    let mut stream = UnixStream::connect(socket).map_err(|e| connect("connect", e))?;
    stream
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .map_err(|e| connect("configure", e))?;
    let mut framed = Vec::with_capacity(4 + payload.len());
    put_string(&mut framed, payload);
    stream
        .write_all(&framed)
        .map_err(|e| connect("write to", e))?;
    let mut reader = Deadline {
        stream: &mut stream,
        until: Instant::now() + timeout,
    };
    read_frame(&mut reader).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData => RequestError::Frame(format!("reply from {at}: {e}")),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset => {
            RequestError::NoAnswer(format!("{at} hung up without replying"))
        }
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            RequestError::NoAnswer(format!("{at} did not reply within {}s", timeout.as_secs()))
        }
        _ => connect("read from", e),
    })
}

/// Reads that share one deadline. A read timeout alone bounds each `read`
/// call, so an agent trickling a byte at a time could stretch a reply
/// without end; this re-arms the timeout with only the time left.
struct Deadline<'a> {
    stream: &'a mut UnixStream,
    until: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match self.stream.set_read_timeout(Some(left)) {
            // macOS refuses the option once the peer has closed (EINVAL); a
            // closed peer cannot stall the read, which returns what is
            // buffered or EOF at once.
            Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {}
            r => r?,
        }
        self.stream.read(buf)
    }
}

/// One length-prefixed agent message; a length out of range is `InvalidData`.
fn read_frame(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("agent message length {len} out of range"),
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

/// Append an SSH `string`: a big-endian u32 length, then the bytes.
fn put_string(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

/// A cursor over SSH wire-format fields.
struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.buf.len() < n {
            bail!("agent message truncated");
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }

    fn string(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_keypair::Keypair;
    use std::os::unix::net::UnixListener;
    use std::thread::JoinHandle;

    /// How a fake agent answers.
    #[derive(Clone, Copy)]
    enum Answer {
        Sign,
        Refuse,
        /// Sign the message with one byte flipped — a signature that does not
        /// verify over what was asked.
        Corrupt,
        /// Close the connection without replying to anything.
        HangUp,
        /// Refuse identity listings (answer `SSH_AGENT_FAILURE`).
        RefuseList,
    }

    /// A fake agent serving `keys` (plus an RSA-typed identity it never signs
    /// with) for `connections` requests, on a socket in its own tempdir.
    struct FakeAgent {
        _dir: tempfile::TempDir,
        path: PathBuf,
        handle: Option<JoinHandle<()>>,
    }

    impl FakeAgent {
        fn spawn(keys: Vec<Keypair>, answer: Answer, connections: usize) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("agent.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let handle = std::thread::spawn(move || {
                for _ in 0..connections {
                    let (mut s, _) = listener.accept().unwrap();
                    let req = read_frame(&mut s).unwrap();
                    if matches!(answer, Answer::HangUp) {
                        continue;
                    }
                    let reply = respond(&keys, answer, &req);
                    let mut framed = Vec::new();
                    put_string(&mut framed, &reply);
                    s.write_all(&framed).unwrap();
                }
            });
            Self {
                _dir: dir,
                path,
                handle: Some(handle),
            }
        }
    }

    impl Drop for FakeAgent {
        fn drop(&mut self) {
            if let Some(h) = self.handle.take() {
                if !std::thread::panicking() {
                    h.join().unwrap();
                }
            }
        }
    }

    fn rsa_identity() -> Identity {
        let mut blob = Vec::new();
        put_string(&mut blob, b"ssh-rsa");
        put_string(&mut blob, &[1, 0, 1]);
        put_string(&mut blob, &[0xab; 16]);
        Identity {
            blob,
            comment: "rsa".into(),
        }
    }

    fn respond(keys: &[Keypair], answer: Answer, req: &[u8]) -> Vec<u8> {
        let mut r = Reader::new(req);
        match r.byte().unwrap() {
            AGENTC_REQUEST_IDENTITIES if matches!(answer, Answer::RefuseList) => {
                vec![AGENT_FAILURE]
            }
            AGENTC_REQUEST_IDENTITIES => {
                let mut ids = vec![rsa_identity()];
                ids.extend(keys.iter().map(|k| Identity {
                    blob: ed25519_blob(&k.pubkey()),
                    comment: format!("item {}", k.pubkey()),
                }));
                let mut out = vec![AGENT_IDENTITIES_ANSWER];
                out.extend_from_slice(&(ids.len() as u32).to_be_bytes());
                for id in ids {
                    put_string(&mut out, &id.blob);
                    put_string(&mut out, id.comment.as_bytes());
                }
                out
            }
            AGENTC_SIGN_REQUEST => {
                let blob = r.string().unwrap();
                let mut data = r.string().unwrap().to_vec();
                assert_eq!(r.u32().unwrap(), 0);
                assert!(r.is_empty());
                let Some(key) = keys.iter().find(|k| ed25519_blob(&k.pubkey()) == blob) else {
                    return vec![AGENT_FAILURE];
                };
                match answer {
                    Answer::Refuse => return vec![AGENT_FAILURE],
                    Answer::Corrupt => data[0] ^= 1,
                    Answer::Sign | Answer::HangUp | Answer::RefuseList => {}
                }
                let mut sig = Vec::new();
                put_string(&mut sig, ED25519);
                put_string(&mut sig, key.sign_message(&data).as_ref());
                let mut out = vec![AGENT_SIGN_RESPONSE];
                put_string(&mut out, &sig);
                out
            }
            t => panic!("fake agent got message type {t}"),
        }
    }

    #[test]
    fn candidates_follow_ssh_then_1password_order() {
        let got = candidate_sockets(Some("/run/ssh.sock".into()), Some("/Users/op".into()));
        assert_eq!(
            got,
            [
                PathBuf::from("/run/ssh.sock"),
                PathBuf::from("/Users/op/.1password/agent.sock"),
                PathBuf::from(
                    "/Users/op/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"
                ),
            ]
        );
        // SSH_AUTH_SOCK already pointing at the symlink is not tried twice.
        let got = candidate_sockets(
            Some("/Users/op/.1password/agent.sock".into()),
            Some("/Users/op".into()),
        );
        assert_eq!(got.len(), 2);
        assert!(candidate_sockets(Some("".into()), None).is_empty());
    }

    #[test]
    fn identities_decode_and_only_ed25519_has_an_address() {
        let key = Keypair::new();
        let agent = FakeAgent::spawn(vec![key.insecure_clone()], Answer::Sign, 1);
        let ids = list_identities(&agent.path).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].key_type(), "ssh-rsa");
        assert_eq!(ids[0].ed25519_pubkey(), None);
        assert_eq!(ids[1].key_type(), "ssh-ed25519");
        assert_eq!(ids[1].ed25519_pubkey(), Some(key.pubkey()));
        assert_eq!(ids[1].comment, format!("item {}", key.pubkey()));
    }

    #[test]
    fn connect_falls_through_to_the_agent_serving_the_key() {
        let key = Keypair::new();
        let other = FakeAgent::spawn(vec![Keypair::new()], Answer::Sign, 1);
        let right = FakeAgent::spawn(vec![key.insecure_clone()], Answer::Sign, 1);
        let missing = PathBuf::from("/nonexistent/agent.sock");
        let signer = AgentSigner::connect_among(
            key.pubkey(),
            &[missing, other.path.clone(), right.path.clone()],
        )
        .unwrap();
        assert_eq!(signer.socket(), right.path);
        assert_eq!(signer.pubkey(), key.pubkey());
        assert!(signer.is_interactive());
    }

    #[test]
    fn connect_reports_every_socket_when_none_serves_the_key() {
        let agent = FakeAgent::spawn(vec![Keypair::new()], Answer::Sign, 1);
        let want = Keypair::new().pubkey();
        let err = AgentSigner::connect_among(want, std::slice::from_ref(&agent.path)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(&want.to_string()), "{msg}");
        assert!(msg.contains("2 identities"), "{msg}");
        assert!(AgentSigner::connect_among(want, &[]).is_err());
    }

    #[test]
    fn signs_a_transaction_that_verifies() {
        use solana_instruction::{AccountMeta, Instruction};
        use solana_transaction::Transaction;

        let key = Keypair::new();
        // One listing for connect, one sign request.
        let agent = FakeAgent::spawn(vec![key.insecure_clone()], Answer::Sign, 2);
        let signer = AgentSigner::connect(key.pubkey(), Some(&agent.path)).unwrap();
        let ix = Instruction::new_with_bytes(
            Pubkey::new_unique(),
            &[1, 2, 3],
            vec![AccountMeta::new(key.pubkey(), true)],
        );
        let mut tx = Transaction::new_with_payer(&[ix], Some(&key.pubkey()));
        tx.try_sign(&[&signer], solana_message::Hash::new_from_array([7; 32]))
            .unwrap();
        assert!(tx.signatures[0].verify(key.pubkey().as_ref(), &tx.message_data()));
        // Ed25519 is deterministic: the agent's signature is the keypair's.
        assert_eq!(tx.signatures[0], key.sign_message(&tx.message_data()));
    }

    #[test]
    fn a_refusal_is_a_user_cancel() {
        let key = Keypair::new();
        let agent = FakeAgent::spawn(vec![key.insecure_clone()], Answer::Refuse, 1);
        let signer = AgentSigner {
            socket: agent.path.clone(),
            pubkey: key.pubkey(),
        };
        assert!(matches!(
            signer.try_sign_message(b"msg"),
            Err(SignerError::UserCancel(_))
        ));
    }

    #[test]
    fn a_signature_that_does_not_verify_is_rejected() {
        let key = Keypair::new();
        let agent = FakeAgent::spawn(vec![key.insecure_clone()], Answer::Corrupt, 1);
        let signer = AgentSigner {
            socket: agent.path.clone(),
            pubkey: key.pubkey(),
        };
        match signer.try_sign_message(b"msg") {
            Err(SignerError::Protocol(m)) => assert!(m.contains("does not verify"), "{m}"),
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[test]
    fn a_dead_socket_is_a_connection_error() {
        let signer = AgentSigner {
            socket: "/nonexistent/agent.sock".into(),
            pubkey: Pubkey::new_unique(),
        };
        assert!(matches!(
            signer.try_sign_message(b"msg"),
            Err(SignerError::Connection(_))
        ));
    }

    #[test]
    fn a_hang_up_is_a_user_cancel_not_a_connection_error() {
        let key = Keypair::new();
        let agent = FakeAgent::spawn(vec![key.insecure_clone()], Answer::HangUp, 1);
        let signer = AgentSigner {
            socket: agent.path.clone(),
            pubkey: key.pubkey(),
        };
        match signer.try_sign_message(b"msg") {
            Err(SignerError::UserCancel(m)) => assert!(m.contains("hung up"), "{m}"),
            other => panic!("expected a user cancel, got {other:?}"),
        }
    }

    #[test]
    fn a_refused_listing_is_recorded_and_skipped() {
        let key = Keypair::new();
        let refusing = FakeAgent::spawn(vec![key.insecure_clone()], Answer::RefuseList, 1);
        let serving = FakeAgent::spawn(vec![key.insecure_clone()], Answer::Sign, 1);
        let signer = AgentSigner::connect_among(
            key.pubkey(),
            &[refusing.path.clone(), serving.path.clone()],
        )
        .unwrap();
        assert_eq!(signer.socket(), serving.path);
        let lonely = FakeAgent::spawn(vec![key.insecure_clone()], Answer::RefuseList, 1);
        let err = AgentSigner::connect_among(key.pubkey(), std::slice::from_ref(&lonely.path))
            .unwrap_err();
        assert!(format!("{err:#}").contains("refused to list"), "{err:#}");
    }

    #[test]
    fn a_frame_length_out_of_range_is_invalid_data() {
        for len in [0u32, MAX_MESSAGE as u32 + 1] {
            let mut bytes = len.to_be_bytes().to_vec();
            bytes.extend_from_slice(&[0; 8]);
            let err = read_frame(&mut bytes.as_slice()).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "len {len}");
        }
    }
}
