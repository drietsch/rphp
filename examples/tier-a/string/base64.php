<?php
// base64.c / quot_print.c / uuencode.c: base64 (lenient and strict),
// quoted-printable, uuencode.

var_dump(base64_encode(""), base64_encode("f"), base64_encode("fo"), base64_encode("foo"), base64_encode("\xff\xfe"), base64_encode("Hello, World! \x00\x01"));
foreach (["Zm9v", "Zm9v\n", " Zm 9v ", "Zm9v=", "Zm9v==", "Zm8=", "Zm8", "Zm8==", "Zm8=x", "Zm8=Zm8=", "Zg==", "Zg=", "Zg", "Z", "Zg===", "=", "==", "Zm9v!", "Zm9v\x00", "Zg==Zg==", "Zm9=v", "Zm9v=\n", "-_", "Z\x80", "Zm9v\x0b", "Zm9v\x0c", "Zm9v\r", "Zm9v\t", "Zm9v\xa0", "Zm==9v", ""] as $s) {
    echo json_encode($s), ": ", json_encode(base64_decode($s)), " ", json_encode(base64_decode($s, true)), "\n";
}
var_dump(bin2hex(base64_decode(base64_encode("\x00\xff\x10\x80abc"))));

// --- quoted-printable ---
var_dump(quoted_printable_encode("hello world=\r\nfoo\nbar\tbaz \r\n"));
var_dump(quoted_printable_encode(str_repeat("a", 80)));
var_dump(quoted_printable_encode("é\x00\x7f\x1f trailing space \nline2 "));
var_dump(quoted_printable_encode("a b\r\n"), quoted_printable_encode("a b \r\n"), quoted_printable_encode("a\r"), quoted_printable_encode("a\n"), quoted_printable_encode(""));
var_dump(quoted_printable_encode(str_repeat("=", 30)));
var_dump(quoted_printable_encode(str_repeat("a", 74) . "\t\n"), quoted_printable_encode(str_repeat("a", 75) . "\t\n"), quoted_printable_encode(str_repeat("a", 73) . "éé"), quoted_printable_encode(str_repeat("a", 70) . "\xe2\x82\xac\xf0\x9f\x98\x80"));
var_dump(quoted_printable_decode("hello=20world=\r\nfoo=\nbar=3D=3d =zz =A =\r\n"));
var_dump(bin2hex(quoted_printable_decode("=C3=A9=")), quoted_printable_decode("a=  \r\nb=  \nc= x=\t\t\r\nd"), quoted_printable_decode("a =\r\n"));
foreach (["a=\rb", "a=\r", "a= \r\nb", "a=  ", "a=_b", "=4", "=4G", "a=\n\nb", "=="] as $s) {
    echo json_encode(quoted_printable_decode($s)), " ";
}
echo "\n";

// --- uuencode ---
var_dump(convert_uuencode("test\ntext text\r\n"), convert_uuencode(""), convert_uuencode("a"), convert_uuencode(str_repeat("x", 45)), convert_uuencode(str_repeat("x", 46)), convert_uuencode(str_repeat("\xff", 91)));
var_dump(convert_uudecode(convert_uuencode("test\ntext text\r\n")), convert_uudecode(convert_uuencode(str_repeat("\xff", 91))) === str_repeat("\xff", 91), convert_uudecode("!!!!\n`\n") === "\x00", convert_uudecode("`\n"));
var_dump(convert_uudecode("0V%T"));        // warning: not valid
var_dump(convert_uudecode(""));            // warning
var_dump(convert_uudecode("0V%T\n0V%T\n`\n"));  // warning
