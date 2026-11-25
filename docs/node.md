# The node

`dropset-node` (the `node/` crate) is the Dropset node: an always-on
process. Today it does nothing useful on purpose. It exists so that the
deploy path, the logs and the stop signal can be proven on a program
that cannot fail for its own reasons. It reads no settings, opens no
connection and needs no secret.

## What it prints

One line to standard output per stage of its life:

| Stage    | Line                                        |
| -------- | ------------------------------------------- |
| Start    | `dropset-node <version> <build> started`    |
| Alive    | `dropset-node heartbeat <n>`, once a minute |
| Shutdown | `dropset-node shutting down`                |

- `<version>` is the crate version.
- `<build>` is the full git commit SHA the binary was built from, baked
  in at compile time from the `DROPSET_NODE_BUILD` environment variable.
  It reads `unknown` when that is absent or empty.
- `<n>` counts the heartbeats from 1.
- The shutdown line prints on the terminate signal or the interrupt
  signal, at once rather than at the next heartbeat, and the process
  then exits with success. Any other exit is a failure: the node exits
  non-zero, without the shutdown line.

Run it locally with `cargo run -p dropset-node`; Ctrl-C stops it.

## The image

`infra/node/Dockerfile` builds the image from the repo root as its
context. It follows the localnet images' pattern: the pinned toolchain
in its own early layer, cargo-chef so dependencies build once and cache
apart from the source, and a slim Debian runtime with certificates
only.

- Both base images come from Docker Hub, pinned by the digest of their
  multi-arch index, and the image is built for ARM (`linux/arm64`).
- The `BUILD_COMMIT` build argument, the full commit SHA, becomes the
  start line's `<build>`.
- The node runs as the non-root `dropset` user (uid 10001), as the
  container's first process, so the host's stop signal reaches it
  directly.
- Its build caches are its own (`node-reg`, `node-git`).

The image is built by the AWS stack's workflow and first run on AWS. It
is not part of the localnet compose project, and no make target builds
or runs it.
