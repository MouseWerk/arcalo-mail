-- Which address each OpenPGP key is used for. An imported key serves every address of its user
-- ids; an Autocrypt key only the address whose mail carried it, and only while no newer mail
-- brought another one (`seen_at`, the message's date). An Autocrypt key never outranks a key the
-- user imported or found in a Web Key Directory.
CREATE TABLE pgp_key_emails (
  fingerprint TEXT NOT NULL,
  email TEXT NOT NULL,
  source TEXT NOT NULL,
  prefer_encrypt INTEGER NOT NULL DEFAULT 0,
  seen_at INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (fingerprint, email)
);
CREATE INDEX pgp_key_emails_email ON pgp_key_emails (email);
INSERT INTO pgp_key_emails (fingerprint, email, source, prefer_encrypt, seen_at)
  SELECT fingerprint, email, source, prefer_encrypt, updated_at FROM pgp_keys;

-- S/MIME: a certificate is used to encrypt only when it chains to a trusted root or the user
-- trusted it. Certificates kept from mail before this was checked wait for the next signed
-- message of their sender.
ALTER TABLE smime_certs ADD COLUMN trusted INTEGER NOT NULL DEFAULT 0;
UPDATE smime_certs SET trusted = 1 WHERE own = 1;

-- The message passed DMARC for the domain of its From, as the user's own provider wrote in its
-- `Authentication-Results`: only then does the sender's BIMI logo show.
ALTER TABLE messages ADD COLUMN dmarc_pass INTEGER NOT NULL DEFAULT 0;
