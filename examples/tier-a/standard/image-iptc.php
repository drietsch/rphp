<?php
// iptcparse() and iptcembed().
function tag($rec, $ds, $value) {
    $n = strlen($value);
    $h = chr(0x1c) . chr($rec) . chr($ds);
    $h .= $n < 0x8000 ? pack('n', $n) : "\x80\x04" . pack('N', $n);
    return $h . $value;
}
$iptc = tag(2, 5, 'Title') . tag(2, 25, 'foo') . tag(2, 25, 'bar') . tag(1, 90, "\x1b%G") . tag(2, 120, str_repeat('c', 40000));
$p = iptcparse($iptc);
foreach ($p as $k => $vals) {
    echo $k, ': ', count($vals), ' ', json_encode(array_map(fn($v) => strlen($v) > 20 ? strlen($v) : $v, $vals)), "\n";
}
var_dump(iptcparse('garbage'));
var_dump(iptcparse(''));
var_dump(iptcparse("xx\x1c\x02\x05\x00\x02ab"));
var_dump(iptcparse("\x1c\x02\x05\x00\x09ab"));
var_dump(iptcparse("\x1c\x02\x05\x80\x04\x00\x00\x00\x02ab"));
var_dump(iptcparse("\x1c\x02\x05\x00\x02ab\x00\x1c\x02\x06\x00\x01c"));
var_dump(iptcparse("\x1c\x02"));
var_dump(iptcparse("\x1c\x03\x05\x00\x01a"));
var_dump(iptcparse("\x1c\x02\x05\x00\x01a\x1c\x03\x07\x00\x01b"));

// A minimal JPEG: SOI, APP0, SOF0, SOS, data, EOI.
$jpeg = "\xff\xd8"
    . "\xff\xe0\x00\x10JFIF\0\x01\x01\0\0\x01\0\x01\0\0"
    . "\xff\xc0\x00\x0b\x08\x00\x01\x00\x02\x01\x01\x11\x00"
    . "\xff\xda\x00\x08\x01\x01\x00\x00\x3f\x00" . "\x12\x34\xff\x00\x56"
    . "\xff\xd9";
file_put_contents('image-iptc.jpg', $jpeg);
$out = iptcembed(tag(2, 5, 'Hello') . tag(2, 116, 'odd'), 'image-iptc.jpg');
echo bin2hex($out), "\n";
$r = getimagesizefromstring($out, $info);
echo json_encode($r), "\n", json_encode(array_keys($info)), "\n";
var_dump(iptcparse(substr($info['APP13'], 26)));

// Re-embedding replaces the existing APP13.
file_put_contents('image-iptc2.jpg', $out);
echo bin2hex(iptcembed(tag(2, 5, 'Again'), 'image-iptc2.jpg')), "\n";

// spool 1 echoes and returns the data; spool 2 only echoes.
ob_start();
$r1 = iptcembed(tag(2, 5, 'x'), 'image-iptc.jpg', 1);
$echo1 = ob_get_clean();
var_dump($echo1 === $r1, strlen($r1));
ob_start();
$r2 = iptcembed(tag(2, 5, 'x'), 'image-iptc.jpg', 2);
$echo2 = ob_get_clean();
var_dump($r2, $echo2 === $r1);

// No APP0/APP1: nothing is embedded.
file_put_contents('image-iptc3.jpg', "\xff\xd8\xff\xdb\x00\x03\x00\xff\xd9");
echo bin2hex(iptcembed('abc', 'image-iptc3.jpg')), "\n";
// Not a JPEG.
file_put_contents('image-iptc3.jpg', "-1-1");
var_dump(iptcembed('abc', 'image-iptc3.jpg'));
var_dump(iptcembed('abc', 'image-iptc3.jpg', 1));
file_put_contents('image-iptc3.jpg', "");
var_dump(iptcembed('abc', 'image-iptc3.jpg'));
unlink('image-iptc.jpg');
unlink('image-iptc2.jpg');
unlink('image-iptc3.jpg');
var_dump(iptcembed('abc', 'image-iptc-missing.jpg'));
try {
    iptcembed('abc', "a\0b");
} catch (\ValueError $e) {
    echo $e->getMessage(), "\n";
}
