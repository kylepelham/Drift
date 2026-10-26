# Conversation sync

Status: in progress. Built: the cryptography (`crates/drift-engine/src/cloudsync/crypto.rs`). Planned: the
server, uploading and viewing, continuing a chat, the sidebar and Settings. The plan is the session plan
`conversation-sync.md`.

## What it is for

A chat started on one computer (a long reverse engineering job on the work PC) can be read and carried on from
another (home). Each computer keeps its own chats exactly as they are: another computer's chats open in a
separate, temporary view chosen from a **Computers** button at the top of the sidebar (on the phone, the computer
picker it already has). Removing a computer from sync deletes only the synced copy; the computer keeps its chats
and uploads them again the next time it is online.

## Who can read what

The server stores only sealed objects under random names, with a small index of which computer wrote each one and
in what order. It never sees a title, path, message, workspace name or attachment, and it cannot forge or reorder
what a computer wrote. Whoever runs the server (Cloudflare, or someone who gets into the account) learns only
object sizes (padded to 4 KiB), upload times and how many computers there are.

- **Unlocking.** The user picks a sync password. Argon2id (64 MiB, three passes, a random salt per group) stretches
  it into a master key, and HKDF-SHA256 draws two keys from that: a login key, sent to the server, which stores
  only its SHA-256, and a wrapping key, never sent. The group key (32 random bytes) seals every object; the server
  keeps it wrapped (ChaCha20-Poly1305) by the wrapping key, so only the password opens it.
- **Second factor.** The server hands out the wrapped key only for the right login key and a current TOTP code
  from the user's authenticator app. This stops someone who learned the password; it cannot stop whoever runs the
  server from guessing passwords against the wrapped key offline, which Argon2id makes slow, so the password
  should be long.
- **Remembering a computer.** A computer the user asks to remember keeps the group key and its own signing key in
  the OS keychain (Windows Credential Manager, the macOS Keychain, the Secret Service) and never asks again.
- **Objects.** Each object is a batch of a chat's changes: compressed with zstd (conversation data shrinks about
  3.7 times), padded, and sealed with ChaCha20-Poly1305 under a key HKDF draws from the group key and the object's
  own random 16-byte id, so no key and nonce pair can repeat. The group, the writing computer and its sequence
  number are bound into the seal, so an object moved, replayed or renumbered does not open.
- **Signatures.** Each computer has an Ed25519 key and signs everything it uploads. The device list pairs each
  computer's public key with an HMAC tag only the group key can make, so a server cannot add a key of its own and
  write as a new computer. Readers check both.
- **Attachments** are their own objects, named by an HMAC of their hash under the group key, so the same file is
  stored once and the server cannot tell which known file it is.

## Storage

Measured on Kyle's main computer (October 2026): 4.1 GB of conversation data and 1.5 GB of attachments over 2,937
chats, which compress to about 1.1 GB and 1.5 GB (attachments are images and do not shrink). The last 30 days
added 2.1 GB, about 560 MB compressed. One computer's whole history fits Cloudflare R2's free 10 GB with room for a
second computer; at that pace, two busy computers fill it in about a year, after which R2 costs $0.015 per GB a
month (30 GB is about $0.45 a month). Settings will show what each computer stores and let old chats be left out.

## Continuing another computer's chat

The computer a chat was started on owns it. Opening another computer's chat and sending in it never sends into
that chat: it makes a fork owned by this computer, starting from everything synced so far, so two computers never
run one conversation and never split its prompt cache. Its tools run in the local folder the user mapped once to
that workspace (offered by matching git remote, then by name); an unmapped workspace's chats are read only.
