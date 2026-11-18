# Security

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub: open the repository's **Security** tab and choose **Report a vulnerability**. Don't open a public issue for them. Reports are answered as soon as possible; fixes ship in a new release, with credit if you want it.

## What ClipRust protects

ClipRust keeps your clipboard history in one SQLite database (`~/Library/Application Support/cliprust/cliprust.db`). Everything you copy is stored there, except the contents of copied files, which stay where they are.

- **Saved keys** (API keys and tokens it detects) are always encrypted with XChaCha20-Poly1305. The master key is a generic-password item in your login Keychain (service `app.cliprust`). Their checksums are keyed, so a guessed key can't be confirmed against the file, and the search index never holds them.
- **The whole database** can be encrypted too, with SQLCipher (Settings → Security, or `cliprust encryption on`). Its key is derived from the same master key.
- **Recovery code.** The master key can be shown as a recovery code (behind Touch ID or your password) for a password manager. With it, a database moved to another Mac, or one whose Keychain item was lost, opens again.
- **Rotation** re-encrypts with a new master key and keeps the old one, so older copies of the database still open. Removing old keys is a separate step; older backups then need the recovery code that was current when they were made.

## Threat model

| Threat | Without database encryption | With database encryption |
|---|---|---|
| Database file copied, stolen disk without FileVault, unencrypted Time Machine disk | Saved keys are safe. Other clips, images, titles, notes and masked key hints are readable | Everything is safe while your login Keychain password is unknown |
| Backups or snapshots taken before a key was encrypted, or before database encryption was turned on | Still readable | Still readable |
| Another local user without admin rights | Blocked by the permissions on `~/Library` | Same |
| Malware or scripts running as you | The Keychain asks before a binary other than the one that created the key reads it. They can still read the live clipboard and ClipRust's memory | Same |
| Other clipboard managers | Keys that ClipRust puts on the clipboard are marked concealed | Same |
| Root, an admin, or someone at your unlocked Mac | Not protected | Not protected |
| Secrets ClipRust doesn't detect (passwords, keys inside long clips) | Stored in plaintext | Encrypted with the rest of the file |

## Limits

- Data removed from the database can survive in free disk space, APFS snapshots and Time Machine backups. FileVault protects those; turn it on.
- Revealed keys pass through Swift strings, which can't be wiped from memory.
- On Linux there is no Keychain, so the command-line tool stores keys unencrypted, as before.
- Release builds use the hardened runtime, which blocks casual debugging of the app.
