<?php
// mb_convert_encoding across the codec families, substitute-character
// modes, the array form, from-lists, and mb_scrub / mb_check_encoding.
function hex($s) { return $s === false ? "false" : bin2hex($s); }

echo "-- Unicode forms\n";
$u = "aé日😀";
foreach (["UTF-16", "UTF-16BE", "UTF-16LE", "UTF-32", "UTF-32BE", "UTF-32LE", "UCS-2", "UCS-2BE", "UCS-2LE", "UCS-4", "UCS-4LE", "UTF-7", "UTF7-IMAP", "UTF-8"] as $enc) {
    $conv = mb_convert_encoding($u, $enc, "UTF-8");
    echo str_pad($enc, 10), hex($conv), " -> ", json_encode(mb_convert_encoding($conv, "UTF-8", $enc)), "\n";
}
var_dump(mb_convert_encoding("\xff\xfe\x61\x00", "UTF-8", "UTF-16"), mb_convert_encoding("\xfe\xff\x00\x61", "UTF-8", "UTF-16"), mb_convert_encoding("\x00\x61", "UTF-8", "UTF-16"), mb_convert_encoding("\xff\xfe\x00\x00\x61\x00\x00\x00", "UTF-8", "UTF-32"), mb_convert_encoding("\xff\xfe\x61\x00", "UTF-8", "UCS-2"));
var_dump(mb_convert_encoding("\xd8\x3d", "UTF-8", "UTF-16BE"), mb_convert_encoding("\xdc\x00", "UTF-8", "UTF-16BE"), mb_convert_encoding("\xd8\x3d\x00\x61", "UTF-8", "UTF-16BE"), mb_convert_encoding("\x00\x61\x00", "UTF-8", "UTF-16BE"), mb_convert_encoding("\x00\x00\xd8\x00", "UTF-8", "UTF-32BE"), mb_convert_encoding("\x00\x11\x00\x00", "UTF-8", "UTF-32BE"), hex(mb_convert_encoding("😀", "UCS-2")));
var_dump(mb_convert_encoding("+AOk-1+-a", "UTF-8", "UTF-7"), mb_convert_encoding("+AOk", "UTF-8", "UTF-7"), mb_convert_encoding("+AOkA", "UTF-8", "UTF-7"), mb_convert_encoding("a+b", "UTF-8", "UTF-7"), mb_convert_encoding("&AOk-&-", "UTF-8", "UTF7-IMAP"), mb_convert_encoding("&AOk", "UTF-8", "UTF7-IMAP"));
var_dump(mb_convert_encoding("Hi Mom -☺-!", "UTF-7"), mb_convert_encoding("日本語", "UTF-7"), mb_convert_encoding("A+B", "UTF-7"), mb_convert_encoding("a&b日", "UTF7-IMAP"), mb_convert_encoding("~peter/mail/日本語/台北", "UTF7-IMAP"));

echo "-- invalid UTF-8\n";
foreach (["\xc0\x80", "\xe0\x80\x80", "\xed\xa0\x80", "\xf4\x90\x80\x80", "\xf8\x88\x80\x80\x80", "\xe2\x82", "\xe2\x82a", "\xf0\x9f\x98", "a\x80b", "\xc3", "\xc3\xa9\xc3", "\xef\xbf\xbe", "\xf0\x90\x80"] as $bad) {
    echo hex($bad), " => ", json_encode(mb_convert_encoding($bad, "UTF-8", "UTF-8")), " ", var_export(mb_check_encoding($bad, "UTF-8"), true), " ", mb_strlen($bad), "\n";
}

echo "-- substitute modes\n";
foreach (["none", "long", "entity", 0x2A, 0x1F600, 63] as $mode) {
    mb_substitute_character($mode);
    var_dump(mb_convert_encoding("aé日😀\xff", "ASCII", "UTF-8"), mb_convert_encoding("aé日😀\xff", "ISO-8859-1", "UTF-8"), mb_convert_encoding("a\xffb", "UTF-8", "UTF-8"), mb_convert_encoding("aé日😀", "UTF-16BE") === mb_convert_encoding("aé日😀", "UTF-16BE", "UTF-8"));
}
mb_substitute_character(63);

echo "-- single-byte charsets\n";
foreach (["ISO-8859-1", "ISO-8859-2", "ISO-8859-5", "ISO-8859-7", "ISO-8859-9", "ISO-8859-15", "Windows-1252", "Windows-1251", "Windows-1254", "KOI8-R", "KOI8-U", "CP866", "CP850", "ArmSCII-8", "ASCII", "8bit", "7bit"] as $enc) {
    $t = "aé€Ωжł₴հ(x)";
    $c = mb_convert_encoding($t, $enc, "UTF-8");
    echo str_pad($enc, 13), hex($c), " ", json_encode(mb_convert_encoding($c, "UTF-8", $enc)), "\n";
}
var_dump(mb_convert_encoding("\x80\x81\x8d\x9f", "UTF-8", "Windows-1252"), hex(mb_convert_encoding("\u{81}\u{20ac}", "Windows-1252", "UTF-8")), mb_convert_encoding("\xa1\xa2\xff", "UTF-8", "ISO-8859-2"), hex(mb_convert_encoding("\x00\x80\xff", "ISO-8859-1", "8bit")));

echo "-- CJK\n";
foreach (["SJIS", "CP932", "EUC-JP", "ISO-2022-JP", "JIS", "GBK", "GB18030", "EUC-CN", "BIG-5", "EUC-KR", "UHC", "ISO-2022-KR", "HZ"] as $enc) {
    $t = "a日本語한中";
    $c = mb_convert_encoding($t, $enc, "UTF-8");
    echo str_pad($enc, 12), hex($c), " ", json_encode(mb_convert_encoding($c, "UTF-8", $enc)), "\n";
}
foreach (["SJIS", "CP932", "EUC-JP", "ISO-2022-JP", "JIS"] as $enc) {
    $c = mb_convert_encoding("あ、ｱ。", $enc, "UTF-8");
    echo str_pad($enc, 12), hex($c), " ", json_encode(mb_convert_encoding($c, "UTF-8", $enc)), "\n";
}
var_dump(mb_convert_encoding("\x82\xa0\x82", "UTF-8", "SJIS"), mb_convert_encoding("\x1b\$B\$\"", "UTF-8", "ISO-2022-JP"), mb_convert_encoding("\x1b\$B\$", "UTF-8", "ISO-2022-JP"), mb_convert_encoding("\x1b(Ia\x1b(B", "UTF-8", "JIS"), mb_convert_encoding("\x1b(J\x5c\x7e\x1b(B", "UTF-8", "ISO-2022-JP"), mb_convert_encoding("~{VP~}x~~", "UTF-8", "HZ"));

echo "-- byte codecs\n";
var_dump(mb_convert_encoding("Hello, World!", "BASE64", "UTF-8"), mb_convert_encoding("SGVsbG8sIFdvcmxkIQ==", "UTF-8", "BASE64"), mb_convert_encoding(str_repeat("abc", 30), "BASE64"), mb_convert_encoding("SGVs\nbG8=", "UTF-8", "BASE64"), mb_convert_encoding("SG*s", "8bit", "BASE64"));
var_dump(mb_convert_encoding("é=x\r\ny", "Quoted-Printable", "UTF-8"), mb_convert_encoding("=C3=A9=3Dx\r\ny=\r\nz", "UTF-8", "Quoted-Printable"), mb_convert_encoding(str_repeat("é", 40), "Quoted-Printable"));
var_dump(mb_convert_encoding("Hello", "UUENCODE"), mb_convert_encoding(mb_convert_encoding(str_repeat("xyz", 40), "UUENCODE"), "UTF-8", "UUENCODE") === str_repeat("xyz", 40), mb_convert_encoding("begin 644 f\n%2&5L;&\\`\n`\nend\n", "8bit", "UUENCODE"));
var_dump(mb_convert_encoding("a&amp;é&#x1F600;&#65;&bogus;&#;&", "UTF-8", "HTML-ENTITIES"), mb_convert_encoding("aé<日😀&\"", "HTML-ENTITIES", "UTF-8"), mb_convert_encoding("&eacute;&#233;&#XE9;&#x110000;", "UTF-8", "HTML"));
var_dump(mb_convert_encoding("aé", "7bit", "UTF-8"), mb_convert_encoding("a\xe9", "UTF-8", "8bit"), mb_convert_encoding("a\xe9", "7bit", "8bit"), mb_convert_encoding("a\xe9", "8bit", "7bit"));

echo "-- from lists and arrays\n";
var_dump(mb_convert_encoding("caf\xe9", "UTF-8", "UTF-8, ISO-8859-1"), mb_convert_encoding("café", "UTF-8", ["ASCII", "UTF-8", "ISO-8859-1"]), mb_convert_encoding("café", "ISO-8859-1", "auto"), mb_convert_encoding("caf\xe9", "UTF-8", "ASCII, BASE64, ISO-8859-1"), mb_convert_encoding("café", "UTF-8", "\"UTF-8\""));
var_dump(mb_convert_encoding(["k" => "é", "n" => [1, 2.5, true, null, "日"], 3 => "x"], "ISO-8859-1", "UTF-8") === ["k" => "\xe9", "n" => [1, 2.5, true, null, "?"], 3 => "x"], mb_convert_encoding(["é" => "é"], "ISO-8859-1") === ["\xe9" => "\xe9"], mb_convert_encoding([], "UTF-8"), mb_convert_encoding("x", "UTF-8", "UTF-8,ASCII"));
var_dump(mb_convert_encoding("\xff\xfe", "UTF-8", "ASCII, UTF-8"), mb_convert_encoding("日本", "UTF-8", "ASCII, JIS, UTF-8, EUC-JP, SJIS"), mb_convert_encoding("\x93\xfa\x96\x7b", "UTF-8", "ASCII, JIS, UTF-8, EUC-JP, SJIS"), mb_convert_encoding("\xc6\xfc\xcb\xdc", "UTF-8", "JIS, UTF-8, EUC-JP, SJIS"));

echo "-- scrub / check\n";
var_dump(mb_scrub("a\xffb"), mb_scrub("日本"), mb_scrub("\x00\x61\x00", "UTF-16BE") === "\x00\x61\x3f", mb_scrub("\xe3\x81", "UTF-8"), mb_scrub("", "SJIS"));
var_dump(mb_check_encoding("日本"), mb_check_encoding("\xff"), mb_check_encoding("\x00\x61", "UTF-16BE"), mb_check_encoding("\x00", "UTF-16BE"), mb_check_encoding("+AOk-", "UTF-7"), mb_check_encoding("+AOk", "UTF-7"), mb_check_encoding("\x1b\$BF|\x1b(B", "ISO-2022-JP"), mb_check_encoding("\x1b\$BF|", "ISO-2022-JP"), mb_check_encoding("\x82\xa0", "SJIS"), mb_check_encoding("\x82", "SJIS"), mb_check_encoding("abc", "ASCII"), mb_check_encoding("\x80", "ASCII"));
var_dump(mb_check_encoding(["日" => ["本", 1, null, true], "x"]), mb_check_encoding(["ok", "\xff"]), mb_check_encoding(["\xff" => "ok"]), mb_check_encoding(["x" => "\x80"], "ISO-8859-1"));
var_dump(mb_get_info("illegal_chars") > 0);

echo "-- variables\n";
$a = "café"; $b = ["x" => "日本", "y" => ["é"]]; $c = 42;
var_dump(mb_convert_variables("ISO-8859-1", "UTF-8", $a, $b, $c), $a === "caf\xe9", $b === ["x" => "??", "y" => ["\xe9"]], $c);
$d = "caf\xe9";
var_dump(mb_convert_variables("UTF-8", "UTF-8, ISO-8859-1", $d), $d, mb_convert_variables("UTF-8", "UTF-8", $d), $d);
