# SFTP delivery draft (M19 part 2)

Not built: russh 0.65 cannot sit next to age 0.12.1 (both pin a different `kem` 0.3). To resume once age publishes its ml-kem 0.3 release: add russh and russh-sftp, move these files into `crates/api/src/certification/`, and wire the `sftp` protocol (host key trusted on first use, then pinned). See PR #209.
