<?php
// Under the CLI there is nowhere to send a header, but php still keeps the
// list — and the moment output reaches the SAPI they count as sent. Output
// held in an `ob_*` level has not been sent, so this all works inside one.
ob_start();
var_dump(headers_sent(), headers_list(), http_response_code());

header('Content-Type: text/plain');
header('X-One: a');
header('X-One: b');
var_dump(headers_list());
header('X-One: c', false);
var_dump(headers_list());

var_dump(http_response_code(201), http_response_code(), http_response_code());
header('HTTP/1.1 404 Not Found');
var_dump(http_response_code());
header_remove('x-one');
var_dump(headers_list());
header_remove();
var_dump(headers_list(), headers_sent());

$buffered = ob_get_clean();
var_dump(strlen($buffered) > 0);

// Once anything is out, every call is php's refusal, and it names where the
// output started.
echo "sent\n";
$file = null;
$line = null;
var_dump(headers_sent(), headers_sent($file, $line), basename($file), $line > 0);
header('X-Late: 1');
var_dump(headers_list());
var_dump(@http_response_code(500));
