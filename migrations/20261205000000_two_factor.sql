-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Two-factor authentication (docs/api.md#two-factor-authentication): an
-- authenticator app, passkeys and recovery codes per user. Which roles
-- require it is in the `two_factor` settings.

-- One authenticator app (TOTP) per user. The secret is needed in clear text
-- to check codes, like the output passwords and webhook secrets.
CREATE TABLE user_totp (
    user_id       BIGINT UNSIGNED NOT NULL,
    secret        VARBINARY(64) NOT NULL,
    -- The last 30-second step accepted: a code is accepted once.
    last_step     BIGINT UNSIGNED NULL,
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    last_used_at  DATETIME(6) NULL,
    PRIMARY KEY (user_id),
    CONSTRAINT user_totp_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Passkeys and security keys (WebAuthn): the credential as webauthn-rs
-- keeps it (public key, sign count…), as JSON.
CREATE TABLE user_webauthn (
    id             BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    user_id        BIGINT UNSIGNED NOT NULL,
    credential_id  VARBINARY(1023) NOT NULL,
    name           VARCHAR(255) NOT NULL,
    passkey        JSON NOT NULL,
    created_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    last_used_at   DATETIME(6) NULL,
    PRIMARY KEY (id),
    UNIQUE KEY user_webauthn_credential (credential_id),
    KEY user_webauthn_user (user_id),
    CONSTRAINT user_webauthn_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Recovery codes: Argon2id hashes, each used once.
CREATE TABLE user_recovery_codes (
    id          BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    user_id     BIGINT UNSIGNED NOT NULL,
    code_hash   VARCHAR(255) NOT NULL,
    used_at     DATETIME(6) NULL,
    created_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    KEY user_recovery_codes_user (user_id),
    CONSTRAINT user_recovery_codes_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
