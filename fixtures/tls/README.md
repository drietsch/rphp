# Certificates for the TLS tests

A throwaway two-certificate chain for `tools/rphp/tests/tls.rs`, which
stands a `rustls` server on loopback and points both php and rphp at it.
**These are test fixtures, not secrets** — `key.pem` is committed on
purpose, and nothing outside this test trusts any of it.

| file | what it is |
|------|------------|
| `ca.pem`   | the trust anchor, passed as the `cafile` ssl option |
| `cert.pem` | the leaf the server presents, signed by that CA |
| `key.pem`  | the leaf's private key |

    ca.pem    CN=rphp test CA          basicConstraints=CA:TRUE
    cert.pem  CN=localhost             subjectAltName=DNS:localhost
    both      notAfter = 2101-09-04

Three details are deliberate:

* **A real chain, not a self-signed leaf.** webpki refuses a certificate
  that is its own trust anchor (`CaUsedAsEndEntity`) where openssl allows
  it, so a self-signed fixture would fail under rphp and pass under php for
  a reason that has nothing to do with the engine.
* **`DNS:localhost` and no IP.** The test connects to `127.0.0.1` for the
  case that checks a name mismatch *is* refused, and to `localhost` for the
  cases that should succeed.
* **The long life**, so the suite does not turn red one day over an expiry.

Regenerate with:

    openssl req -x509 -newkey rsa:2048 -nodes -keyout ca-key.pem -out ca.pem \
        -days 27375 -subj "/CN=rphp test CA" \
        -addext "basicConstraints=critical,CA:TRUE" \
        -addext "keyUsage=critical,keyCertSign,cRLSign"
    openssl req -newkey rsa:2048 -nodes -keyout key.pem -out leaf.csr \
        -subj "/CN=localhost"
    printf "subjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n" > leaf.ext
    openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
        -out cert.pem -days 27375 -extfile leaf.ext

`ca-key.pem` is *not* kept: nothing needs to sign anything again.
