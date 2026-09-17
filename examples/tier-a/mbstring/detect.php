<?php
// mb_detect_encoding: php 8.1+'s demerit scoring, strict mode, list forms,
// byte-codec removal, BOMs, and the detect-order / strict_detection defaults.
$cases = [
    "abc", "", "café", "caf\xe9", "日本語", "\x93\xfa\x96\x7b\x8c\xea", "\xc6\xfc\xcb\xdc\xb8\xec", "\x1b\$BF|K\\8l\x1b(B",
    "+ZeVnLIqe-", "\xef\xbb\xbfabc", "\xfe\xff\x00a", "\xff\xfea\x00", "\xff\xfe\xfd", "a\x80b", "\xe3\x81\x82\xff", "!!!!????", "\x81\x40",
];
$lists = [
    "ASCII, UTF-8", "UTF-8, ISO-8859-1", "ISO-8859-1, UTF-8", "ASCII, JIS, UTF-8, EUC-JP, SJIS", "SJIS, EUC-JP, JIS, UTF-8",
    "UTF-8, UTF-7", "UTF-16BE, UTF-16LE, UTF-8", "auto", "UTF-8, ASCII, BASE64", "Windows-1252, UTF-8", "SJIS", "UTF-8",
];
foreach ($cases as $s) {
    echo json_encode($s), ":\n";
    foreach ($lists as $list) {
        echo "  ", str_pad($list, 34), var_export(mb_detect_encoding($s, $list), true), " strict=", var_export(mb_detect_encoding($s, $list, true), true), "\n";
    }
}

echo "-- defaults and list forms\n";
var_dump(mb_detect_encoding("caf\xe9"), mb_detect_encoding("café"), mb_detect_encoding("abc"), mb_detect_encoding("caf\xe9", null, true), mb_detect_encoding("\xff", ["ASCII", "UTF-8"], true));
var_dump(mb_detect_encoding("日本語", ["ASCII", "UTF-8", "SJIS"]), mb_detect_encoding("日本語", "  UTF-8 ,  SJIS "), mb_detect_encoding("日本語", '"UTF-8,SJIS"'), mb_detect_encoding("abc", "BASE64, UUENCODE"), mb_detect_encoding("caf\xe9", mb_list_encodings()), mb_detect_encoding("日本語", mb_list_encodings()));
mb_detect_order("SJIS, EUC-JP, UTF-8");
var_dump(mb_detect_encoding("日本語"), mb_detect_encoding("\x93\xfa\x96\x7b"), mb_detect_order());
mb_detect_order("ASCII,UTF-8");
ini_set("mbstring.strict_detection", "1");
var_dump(mb_detect_encoding("caf\xe9"), mb_detect_encoding("caf\xe9", "UTF-8, ISO-8859-1"), mb_detect_encoding("caf\xe9", "UTF-8, ISO-8859-1", false));
ini_set("mbstring.strict_detection", "0");
mb_language("ja");
var_dump(mb_convert_encoding("\x93\xfa\x96\x7b", "UTF-8", "auto"), mb_detect_encoding("\x93\xfa\x96\x7b", "auto"), mb_detect_encoding("\x93\xfa\x96\x7b", "auto, UTF-8"), mb_detect_order());
mb_language("neutral");
var_dump(mb_detect_encoding("x", "UTF-8, bogus"));
