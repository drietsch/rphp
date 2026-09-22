<?php
// The TLS surface that reaches nothing: which transports are registered,
// and what the crypto switch does when it cannot do its job. The handshake
// itself is exercised against a real server by tools/rphp/tests/tls.rs.

// php registers ssl and tls beside the plain transports, and the four
// version-named aliases after them.
var_dump(stream_get_transports());

echo "--- enabling crypto needs a method ---\n";
$mem = fopen('php://memory', 'r+');
try {
    stream_socket_enable_crypto($mem, true);
} catch (ValueError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

echo "--- a stream with no socket under it ---\n";
// Enabling says so and fails; disabling says so and succeeds, because
// there was nothing to take off in the first place.
var_dump(stream_socket_enable_crypto($mem, true, STREAM_CRYPTO_METHOD_TLS_CLIENT));
var_dump(stream_socket_enable_crypto($mem, false));
fclose($mem);

echo "--- nothing listening ---\n";
$c = @stream_socket_client('ssl://127.0.0.1:1', $errno, $errstr, 2);
var_dump($c, $errno, $errstr);

echo "--- an address with no port ---\n";
$c = @stream_socket_client('tls://127.0.0.1', $e2, $s2, 2);
var_dump($c, $e2, $s2);
