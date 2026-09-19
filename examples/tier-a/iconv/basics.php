<?php

// What implementation the extension says it is, which is what decides the
// `//TRANSLIT` answers below.
var_dump(ICONV_IMPL, ICONV_VERSION, ICONV_MIME_DECODE_STRICT,
    ICONV_MIME_DECODE_CONTINUE_ON_ERROR);
var_dump(iconv_get_encoding('all'));
var_dump(iconv_get_encoding(), iconv_get_encoding('input_encoding'),
    iconv_get_encoding('bogus'));

$s = "Grüße, Wörld! ☃ é";

// Plain conversions, both ways.
var_dump(bin2hex(iconv('UTF-8', 'ISO-8859-1', 'Grüße')));
var_dump(bin2hex(iconv('ISO-8859-1', 'UTF-8', "\xe4\xf6\xfc")));
var_dump(bin2hex(iconv('UTF-8', 'UTF-16BE', 'ab')));
var_dump(iconv('UTF-8', 'UTF-8', $s) === $s);
var_dump(iconv('', 'UTF-8', 'x'));

// The two suffixes, in both orders and in either case.
var_dump(iconv('UTF-8', 'ASCII//TRANSLIT', 'a "quoted" — dash… ¼ Æ é'));
var_dump(iconv('utf-8', 'ascii//translit', 'é'));
var_dump(iconv('UTF-8', 'ASCII//IGNORE', $s));
var_dump(iconv('UTF-8', 'ASCII//TRANSLIT//IGNORE', "a☃b"));
var_dump(iconv('UTF-8', 'ASCII//IGNORE//TRANSLIT', "a☃b"));

// A character the table cannot approximate is still an error, and so is a
// truncated one — even under //IGNORE, which rescues an illegal byte.
var_dump(iconv('UTF-8', 'ASCII//TRANSLIT', $s));
var_dump(iconv('UTF-8', 'ASCII', $s));
var_dump(iconv('UTF-8', 'ISO-8859-1', "ab\xc3"));
var_dump(iconv('UTF-8', 'ASCII//IGNORE', "a\xffb"));
var_dump(iconv('UTF-8', 'ASCII//IGNORE', "ab\xc3"));
var_dump(iconv('BOGUS', 'UTF-8', 'x'));
var_dump(iconv('UTF-8', 'BOGUS', 'x'));

// The string functions count characters, and report the charset php names.
$u = "Grüße☃";
var_dump(iconv_strlen($u), iconv_strlen($u, 'UTF-8'), strlen($u));
var_dump(iconv_substr($u, 1, 3), iconv_substr($u, -3), iconv_substr($u, 2),
    iconv_substr($u, 10), iconv_substr($u, 0, -2));
var_dump(iconv_strpos($u, 'ü'), iconv_strpos($u, 'x'), iconv_strpos($u, '', 2),
    iconv_strpos($u, 'ß', 3));
var_dump(iconv_strrpos($u, 'ü'), iconv_strrpos($u, 'x'));
var_dump(iconv_strlen("\xff\xfe", 'UTF-8'));
var_dump(iconv_strlen($u, 'BOGUS'));
var_dump(iconv_substr($u, 0, 1, 'BOGUS'));
try {
    iconv_strpos($u, 'ü', -99);
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
var_dump(iconv_strlen(''), iconv_substr('', 0, 1), iconv_strpos('', 'a'));

// MIME: the folding is by input bytes, and a character never straddles two
// encoded words.
$sub = "Präsentation für Übermorgen: a rather long subject line to force folding";
var_dump(iconv_mime_encode('Subject', $sub));
var_dump(iconv_mime_encode('Subject', $sub, ['scheme' => 'Q']));
var_dump(iconv_mime_encode('Subject', 'plain ascii'));
var_dump(iconv_mime_encode('Subject', $sub, ['line-length' => 200]));
var_dump(iconv_mime_encode('Subject', $sub,
    ['scheme' => 'B', 'line-length' => 40, 'line-break-chars' => "\n"]));
$ascii = str_repeat('abcdefghij', 12);
foreach ([30, 50, 76] as $len) {
    foreach (['B', 'Q'] as $scheme) {
        echo strtr(iconv_mime_encode('Subject', $ascii,
            ['scheme' => $scheme, 'line-length' => $len,
             'line-break-chars' => "\n"]), "\n", '|'), "\n";
    }
}
// Every byte php's Q scheme writes as itself.
$printable = '';
for ($i = 0x20; $i <= 0x7e; $i++) {
    $printable .= chr($i);
}
echo iconv_mime_encode('X', $printable, ['scheme' => 'Q', 'line-length' => 10000]), "\n";

var_dump(iconv_mime_decode("=?UTF-8?B?UHLDpHNlbnRhdGlvbg==?="));
var_dump(iconv_mime_decode("=?ISO-8859-1?Q?J=FCrgen?=", 0, 'UTF-8'));
var_dump(iconv_mime_decode("plain text"));
var_dump(iconv_mime_decode("a =?UTF-8?B?w6Q=?= b"));
var_dump(iconv_mime_decode("=?UTF-8?B?w6Q=?= =?UTF-8?B?w6Q=?="));
var_dump(iconv_mime_decode("=?UTF-8?Q?a_b?="));
var_dump(iconv_mime_decode("=?UTF-8?B?!!!?="));
var_dump(iconv_mime_decode("=?BOGUS?Q?x?=", 0, 'UTF-8'));
var_dump(iconv_mime_decode("=?UTF-8?X?abc?="));
var_dump(iconv_mime_decode("=?UTF-8?X?abc?=", ICONV_MIME_DECODE_CONTINUE_ON_ERROR));
var_dump(iconv_mime_decode("Subject: x\r\n\ttab-folded", 0));
var_dump(iconv_mime_decode_headers(
    "Subject: =?UTF-8?B?UHLDpHNlbnRhdGlvbg==?=\r\n"
    . "From: =?ISO-8859-1?Q?J=FCrgen?= <j@example.com>\r\nTo: a@b.c\r\n"));
var_dump(iconv_mime_decode_headers("A: 1\r\nA: 2\r\nB: x\r\n"));
var_dump(iconv_mime_decode_headers("Subject: a\r\n b\r\nX: 1\r\n\r\nbody"));
var_dump(iconv_mime_decode_headers("bogus line\r\nX: 1\r\n"));

// The linear whitespace after an encoded word reaches the output only when
// ordinary text follows it, and a fold after a word disappears entirely.
foreach (["x =?UTF-8?B?w6Q=?= ", "plain text ", "=?UTF-8?B?w6Q=?=  junk",
          "=?UTF-8?B?w6Q=?=\t", "=?UTF-8?B?w6Q=?= \t =?UTF-8?B?w6Q=?=",
          "  =?UTF-8?B?w6Q=?=", "=?UTF-8?B?w6Q=?=\r\n junk",
          "Subject: x\r\n\ttab-folded"] as $raw) {
    printf("%-42s => %s\n", json_encode($raw), json_encode(iconv_mime_decode($raw)));
}

// Strict mode holds a word to RFC 2047's boundaries, but a word that does
// not parse is malformed in either mode.
foreach (["=?UTF-8?B?w6Q=?=x", "=?UTF-8?B?w6Q=?==?UTF-8?B?w6Q=?=",
          "x =?UTF-8?B?w6Q=?= ", "=?UTF-8??w6Q=?=", "=? UTF-8?B?w6Q=?="] as $raw) {
    printf("%-38s => %s\n", json_encode($raw),
        json_encode(iconv_mime_decode($raw, ICONV_MIME_DECODE_STRICT)));
}

// The settings, which php deprecates whole.
var_dump(iconv_set_encoding('internal_encoding', 'ISO-8859-1'));
var_dump(iconv_get_encoding('internal_encoding'), iconv_get_encoding('all'));
var_dump(iconv_strlen("\xe4\xf6"));
var_dump(iconv_set_encoding('input_encoding', 'UTF-8'));
var_dump(iconv_set_encoding('output_encoding', 'UTF-8'));
var_dump(iconv_set_encoding('all', 'UTF-8'));
var_dump(iconv_set_encoding('bogus', 'UTF-8'));
var_dump(iconv_set_encoding('internal_encoding', 'BOGUS'));
var_dump(ini_get('iconv.internal_encoding'));
var_dump(iconv_set_encoding('internal_encoding', 'UTF-8'));
var_dump(iconv_strlen($u));
