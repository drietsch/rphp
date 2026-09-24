<?php
// openssl_verify(): php's three ints and its fourth answer.
//
// The key material is fixed, never generated: ECDSA signing is randomised,
// so a snippet that signed would differ from itself between runs, let alone
// between engines. These are one P-256 and one RSA-2048 key with one
// signature each over "the message", made once by stock php.

$ecPub = base64_decode(
    'LS0tLS1CRUdJTiBQVUJMSUMgS0VZLS0tLS0KTUZrd0V3WUhLb1pJemowQ0FRWUlLb1pJ' .
    'emowREFRY0RRZ0FFMlVDMDN6d3dHdGlSMFo3ckUwMzFvaExiRXhSbgovclRia3BGSWls' .
    'SUJ1UVpqejErUzZBdkprK2hSMVcyZWVWNVhzUnRIc3pYcUJLL0E2V01kNW9IT3h3PT0K' .
    'LS0tLS1FTkQgUFVCTElDIEtFWS0tLS0tCg=='
);

$ecSig = base64_decode(
    'MEYCIQCPrSy/m8kcq82Ml3ntGk1ig53Fsl2ULvTuAhniaW7pcwIhAOj3N4C/A+E9Hh5L' .
    'zadABxnELy9iC0ljcnk/cc0WwIzl'
);

$rsaPub = base64_decode(
    'LS0tLS1CRUdJTiBQVUJMSUMgS0VZLS0tLS0KTUlJQklqQU5CZ2txaGtpRzl3MEJBUUVG' .
    'QUFPQ0FROEFNSUlCQ2dLQ0FRRUEzbEJRK2hKeVdKOW5Qc3UzUnRXdgpGaGN1bEtpeWNP' .
    'NXp1QXBJQ0pTWHRHWm9xUHA5N3g3YlEzRlVCcFIwZ0VZQ0pRTyt6TVRKSVJMNzQwUHZ1' .
    'SktTCnAzcThkMm1wUnR2UnEwVVlFZ25MZmJhQmd1NVNnVk16bU9PdldDdmZyLzBsNXpn' .
    'ZVdZL2ExSTZqVzhTZkczME8KekFUSFVCc3czS1VLWitIc3JDZW05MHR3ZU9MYkI0VDhI' .
    'U2laMEpZaGNpbUhROEZqWmxKN3hFN0E4clB3VVRYKwpzek1uTzRLOERiUGpKRzNZZ3l6' .
    'KzMxY3BhV3BWOUhvNEplQzNTOE9DSjdsT3A0cXA2L2p2aUNuM3djWituTldjCnVBelh2' .
    'THFlOWdvMWU0ektkem1Xa3RWMGROM0JsQW5YVmlGOU9sTHBmMzdmNkY0Q3YzUnhGTWN0' .
    'T0trRWtMdTcKTVFJREFRQUIKLS0tLS1FTkQgUFVCTElDIEtFWS0tLS0tCg=='
);

$rsaSig = base64_decode(
    'NkTLdlszj64aMmBs88Sx3K6mJilnT4Uz91fIhZli3MF+ddQsYJXJINsjlkkRWo3a6mdL' .
    'Y4ie1CUszYymiE91poNYLLQiUj4vsoBcs9WhEj/O9m+dDGso+JJ20Gl0n2TRcQc+Y29s' .
    '03F94s/B+HQ53DiKRkhACVOOjb98n/y4aURXpUxLqli24cphStdlRf+1EyBYZtB3kT06' .
    'N/+Ysu5itgG4+CDVEDg1iYyWJnMpAQfxI6r7Pgk+yMub4Dxyxylkcq5qMrFaToLWeQoQ' .
    'NLO4EAJIKgYZyxNaAlUaOq3pek8x9QHQAgdMUuZ7EzkGMpctPD0QNheptCn+9s2yvpPj' .
    '9w=='
);

// 1 verified, 0 not verified, -1 could not be attempted.
var_dump(openssl_verify('the message', $ecSig, $ecPub, OPENSSL_ALGO_SHA256));
var_dump(openssl_verify('another message', $ecSig, $ecPub, OPENSSL_ALGO_SHA256));
var_dump(openssl_verify('the message', 'not a signature', $ecPub, OPENSSL_ALGO_SHA256));
var_dump(openssl_verify('the message', $rsaSig, $rsaPub, OPENSSL_ALGO_SHA256));
var_dump(openssl_verify('the message', $rsaSig, $rsaPub, OPENSSL_ALGO_SHA1));

// A curve answers for a digest of any length: the prehash is truncated to
// the field, so SHA-512 under a P-256 key is a verdict, not an error.
var_dump(openssl_verify('the message', $ecSig, $ecPub, OPENSSL_ALGO_SHA512));

// The digest by name, and OpenSSL's alias form.
var_dump(openssl_verify('the message', $ecSig, $ecPub, 'sha256'));
var_dump(openssl_verify('the message', $ecSig, $ecPub, 'RSA-SHA256'));

// MD4 is declared by php and gone from OpenSSL 3's default providers.
var_dump(openssl_verify('the message', $rsaSig, $rsaPub, OPENSSL_ALGO_MD4));
while (($e = openssl_error_string()) !== false) {
    echo 'queued: ', $e, "\n";
}

// SHA-3 has no php constant; it is reachable by name, as OpenSSL names it.
var_dump(openssl_verify('the message', $ecSig, $ecPub, 'sha3-256'));

// Not a digest at all: a warning, and false.
var_dump(openssl_verify('the message', $ecSig, $ecPub, 'nonesuch'));
var_dump(openssl_verify('the message', $ecSig, $ecPub, 4711));

// A key that cannot be read: a warning, false, and the decoder's error.
var_dump(openssl_verify('the message', $ecSig, 'not a key', OPENSSL_ALGO_SHA256));
var_dump(openssl_error_string());
var_dump(openssl_error_string());

// The handle form, and what refusing to make one looks like.
$key = openssl_pkey_get_public($ecPub);
var_dump($key instanceof OpenSSLAsymmetricKey);
var_dump(openssl_verify('the message', $ecSig, $key, OPENSSL_ALGO_SHA256));
var_dump(openssl_pkey_get_public('not a key'));
var_dump(openssl_error_string());
