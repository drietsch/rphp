<?php
// convert.quoted-printable-encode / -decode: soft line breaks, trailing
// whitespace, the binary and force-encode-first options, php's line-break
// detection when decoding, and escapes cut across writes.

function qp(string $filter, $params, array $pieces, bool $read = false): string {
    $m = fopen('php://memory', 'w+');
    if ($read) {
        fwrite($m, implode('', $pieces));
        rewind($m);
        $params === null
            ? stream_filter_append($m, $filter, STREAM_FILTER_READ)
            : stream_filter_append($m, $filter, STREAM_FILTER_READ, $params);
        $out = stream_get_contents($m);
        fclose($m);
        return $out;
    }
    $f = $params === null
        ? stream_filter_append($m, $filter, STREAM_FILTER_WRITE)
        : stream_filter_append($m, $filter, STREAM_FILTER_WRITE, $params);
    foreach ($pieces as $p) {
        $n = fwrite($m, $p);
        if ($n !== strlen($p)) {
            echo "  fwrite: ", var_export($n, true), "\n";
        }
    }
    stream_filter_remove($f);
    rewind($m);
    return stream_get_contents($m);
}

echo "--- encode ---\n";
var_dump(qp('convert.quoted-printable-encode', null, ["h\xe9llo = world\r\nline two\t\r\n", str_repeat('a', 30)]));
var_dump(qp('convert.quoted-printable-encode', null, ["a\x00b\x7f", " ", "\n"]));
foreach (["abc   \r\ndef \t", "a \nb", "abc \n", "a\t\nb"] as $s) {
    echo json_encode($s), "\n";
    var_dump(qp('convert.quoted-printable-encode', ['line-length' => 6, 'line-break-chars' => "\n"], [$s], true));
    var_dump(qp('convert.quoted-printable-encode', ['line-break-chars' => "\n"], [$s], true));
}

echo "--- soft line breaks ---\n";
var_dump(qp('convert.quoted-printable-encode', ['line-length' => 20, 'line-break-chars' => "\r\n"], [str_repeat('abc=', 12)]));
var_dump(qp('convert.quoted-printable-encode', ['line-length' => 10], [str_repeat('x', 35)], true));
var_dump(qp('convert.quoted-printable-encode', ['line-length' => 10, 'line-break-chars' => "\n"], [str_repeat("\xff", 12)], true));
foreach ([str_repeat('a', 74) . " b", str_repeat('a', 76), str_repeat('a', 77)] as $s) {
    var_dump(qp('convert.quoted-printable-encode', ['line-length' => 76], [$s], true));
}
var_dump(qp('convert.quoted-printable-encode', ['line-length' => 6, 'line-break-chars' => "\n"], [str_repeat('a', 8) . "\n" . str_repeat('b', 12)], true));

echo "--- binary and force-encode-first ---\n";
var_dump(qp('convert.quoted-printable-encode', ['binary' => true], ["a\r\nb\n c"]));
var_dump(qp('convert.quoted-printable-encode', ['binary' => 'yes', 'line-length' => 8, 'line-break-chars' => "\n"], ["a b\nc d e f g"]));
var_dump(qp('convert.quoted-printable-encode', ['force-encode-first' => true], ["abc\nFrom x"]));
var_dump(qp('convert.quoted-printable-encode', ['force-encode-first' => true, 'line-length' => 8, 'line-break-chars' => "\n"], ["ab.\n.cd\nFrom zzzzzzzz"]));

echo "--- decode ---\n";
foreach ([
    ["h=E9llo =3D=\r\n world", "=4", "1 end=\n"],
    ["=4a=4A=a4"],
    ["a=\rb", "c=\nd", "e=\r\nf"],
    ["a  \r\nb", "a\rb"],
    ["=", "3", "D"],
    ["=  \nx"],
    ["abc="],
    ["abc=4"],
] as $pieces) {
    echo json_encode($pieces), " => ";
    var_dump(qp('convert.quoted-printable-decode', null, $pieces));
}
echo "with line-break-chars:\n";
var_dump(qp('convert.quoted-printable-decode', ['line-break-chars' => "\n"], ["=  \nx", "=\ny"]));
var_dump(qp('convert.quoted-printable-decode', ['line-break-chars' => "\r\n"], ["a=\r", "\nb"]));

echo "--- invalid escapes ---\n";
var_dump(qp('convert.quoted-printable-decode', null, ["bad =ZZ"]));
var_dump(qp('convert.quoted-printable-decode', null, ["=  \r\nx"]));
var_dump(qp('convert.quoted-printable-decode', ['line-break-chars' => "\n"], ["=\r\nx"]));

echo "--- round trip at every cut ---\n";
$text = "Grüße = \"quoted\"\tprintable\r\nsecond line   \r\n" . str_repeat("long ", 30);
$encoded = qp('convert.quoted-printable-encode', null, [$text]);
$ok = true;
for ($n = 1; $n <= 9; $n++) {
    $ok = $ok && qp('convert.quoted-printable-decode', null, str_split($encoded, $n)) === $text;
    $ok = $ok && qp('convert.quoted-printable-encode', null, str_split($text, $n)) === $encoded;
}
var_dump($ok, quoted_printable_decode($encoded) === $text);
