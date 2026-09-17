<?php
// mb_strlen / mb_substr / mb_strcut / mb_str_split / mb_str_pad / mb_trim family,
// the search functions, and the settings functions — UTF-8 unless stated.
$s = "日本語テキスト";
$mixed = "aé日😀z";

echo "-- lengths\n";
var_dump(mb_strlen($s), mb_strlen($mixed), mb_strlen(""), mb_strlen($s, "8bit"), mb_strlen($s, "ASCII"));
var_dump(mb_strlen("a\xffb"), mb_strlen("\xe3\x81", "UTF-8"), mb_strlen("\x00\x61\x00\x62", "UTF-16"), mb_strlen("ab", "UCS-4"));

echo "-- substr\n";
foreach ([[0, null], [1, 2], [-2, null], [-3, 2], [2, -1], [10, 1], [-10, 2], [0, 0], [3, -10]] as [$a, $b]) {
    var_dump(mb_substr($s, $a, $b));
}
var_dump(mb_substr($mixed, 1, 3), mb_substr($mixed, -2), mb_substr("a\xffb\xffc", 1, 3), mb_substr("\x00\x61\x00\x62\x00\x63", 1, 1, "UTF-16BE"), mb_substr("abc", 1, 1, "ASCII"));
var_dump(mb_substr("", 0, 1), mb_substr("abc", 3), mb_substr("abc", -1, -1));

echo "-- strcut\n";
var_dump(mb_strcut($s, 1, 4), mb_strcut($s, 3, 4), mb_strcut($s, 0, 5), mb_strcut($s, -3), mb_strcut($s, 2, -2), mb_strcut("abc", 5), mb_strcut($mixed, 2, 3));
var_dump(bin2hex(mb_strcut("\x00\x61\xd8\x3d\xde\x00\x00\x62", 1, 4, "UTF-16BE")), mb_strcut("\x93\xfa\x96\x7b\x8c\xea", 1, 3, "SJIS") === "\x96\x7b", mb_strcut("abcdef", 1, 3, "8bit"));

echo "-- str_split\n";
var_dump(mb_str_split($s), mb_str_split($s, 3), mb_str_split($mixed, 2), mb_str_split("", 2), mb_str_split("abc", 2, "8bit"), mb_str_split("\x00\x61\x00\x62\x00\x63", 2, "UCS-2"), mb_str_split("a\xffb", 2));

echo "-- str_pad\n";
var_dump(mb_str_pad("日本", 5, "ー"), mb_str_pad("日本", 5, "ー", STR_PAD_LEFT), mb_str_pad("日本", 7, "ーx", STR_PAD_BOTH), mb_str_pad("日本", 1), mb_str_pad("日本", 6, "本語", STR_PAD_BOTH), mb_str_pad("x", 3, "日本語"));

echo "-- trim\n";
var_dump(mb_trim("\u{3000} 日本 \u{a0}\n"), mb_ltrim("\u{3000} 日本 \u{3000}"), mb_rtrim("\u{3000} 日本 \u{3000}"), mb_trim("日本語日", "日"), mb_trim("日本語日", "日語"), mb_trim("xx", "x"), mb_trim("abc", ""), mb_trim(""), mb_trim("\u{85}\u{180e}x\u{200a}\u{2029}"));

echo "-- strpos family\n";
$h = "日本語日本語";
var_dump(mb_strpos($h, "本"), mb_strpos($h, "本", 2), mb_strpos($h, "本", -2), mb_strpos($h, "x"), mb_strpos($h, ""), mb_strpos($h, "", 6), mb_strpos($h, "語", -1));
var_dump(mb_strrpos($h, "本"), mb_strrpos($h, "本", 2), mb_strrpos($h, "本", -2), mb_strrpos($h, "本", -3), mb_strrpos($h, "本", -4), mb_strrpos($h, "x"), mb_strrpos($h, "日本語", -1));
var_dump(mb_stripos("ÄBC ÄBC", "äb"), mb_stripos("ÄBC ÄBC", "äb", 1), mb_strripos("ÄBC ÄBC", "äb"), mb_stripos("straße", "SS"), mb_strripos("ǅa ǆa", "Ǆa"), mb_stripos("abc", "B", -1));
var_dump(mb_strstr($h, "語"), mb_strstr($h, "語", true), mb_strrchr($h, "本"), mb_strrchr($h, "本", true), mb_stristr("ÄBC", "b"), mb_stristr("ÄBC", "b", true), mb_strrichr("ÄbcÄBC", "Ä"), mb_strrichr("ÄbcÄBC", "ä", true), mb_strstr($h, "x"), mb_strrchr($h, "x"));
var_dump(mb_substr_count($h, "本"), mb_substr_count($h, "日本語"), mb_substr_count("aaa", "aa"), mb_substr_count("", "a"), mb_substr_count("\x00\x61\x00\x61", "\x00\x61", "UTF-16BE"));
var_dump(mb_strpos("\x93\xfa\x96\x7b", "\x96\x7b", 0, "SJIS"), mb_strpos("a\xffb", "b"), mb_strpos("a\xffb", "\xff"));

echo "-- settings\n";
var_dump(mb_internal_encoding(), mb_internal_encoding("ISO-8859-1"), mb_internal_encoding(), mb_strlen("\xe9\xe9"), mb_internal_encoding("UTF-8"));
var_dump(mb_language(), mb_language("ja"), mb_language(), mb_language("neutral"), mb_language("uni"), mb_language());
var_dump(mb_http_output(), mb_http_output("SJIS"), mb_http_output(), mb_http_output("pass"), mb_http_output(), mb_http_output("UTF-8"));
var_dump(mb_http_input(), mb_http_input("I"), mb_http_input("L"), mb_http_input("G"), mb_http_input("p"));
var_dump(mb_detect_order(), mb_detect_order("SJIS, UTF-8"), mb_detect_order(), mb_detect_order(["ASCII", "auto"]), mb_detect_order(), mb_detect_order("ASCII,UTF-8"));
var_dump(mb_substitute_character(), mb_substitute_character("none"), mb_substitute_character(), mb_substitute_character("LONG"), mb_substitute_character("entity"), mb_substitute_character(0x1F600), mb_substitute_character(), mb_substitute_character(63));
var_dump(mb_preferred_mime_name("SJIS"), mb_preferred_mime_name("utf8"), mb_preferred_mime_name("ISO-8859-1"), mb_preferred_mime_name("UTF7-IMAP"));
$info = mb_get_info();
var_dump(count($info), $info["internal_encoding"], $info["mail_charset"], $info["mail_header_encoding"], $info["illegal_chars"], $info["encoding_translation"], $info["language"], $info["detect_order"], $info["substitute_character"], $info["strict_detection"], $info["http_output_conv_mimetypes"]);
var_dump(mb_get_info("internal_encoding"), mb_get_info("language"), mb_get_info("detect_order"), mb_get_info("http_input"), mb_get_info("bogus"));
var_dump(mb_parse_str("a=%E6%97%A5&b[]=x&b[]=y", $r), $r, mb_get_info("http_input"), mb_http_input());
var_dump(mb_list_encodings() === array_values(mb_list_encodings()), count(mb_list_encodings()), mb_list_encodings()[18], mb_encoding_aliases("UTF-8"), mb_encoding_aliases("ascii"), mb_encoding_aliases("UTF-16BE"), mb_encoding_aliases("cp936"));
var_dump(mb_ord("日"), mb_ord("😀"), mb_ord("a"), mb_ord("\xff"), mb_ord("\x00\x41", "UTF-16BE"), mb_ord("\x82\xa0", "SJIS"), mb_ord("é", "ISO-8859-1"));
var_dump(mb_chr(0x65E5), mb_chr(0x1F600), mb_chr(65), mb_chr(0xD800), mb_chr(0x110000), mb_chr(-1), bin2hex(mb_chr(0x41, "UTF-16BE")), mb_chr(0x3042, "SJIS") === "\x82\xa0", mb_chr(0x65E5, "ISO-8859-1"), mb_chr(0xE9, "ISO-8859-1") === "\xe9");
var_dump(mb_ucfirst("élan vital"), mb_lcfirst("Élan Vital"), mb_ucfirst("ǆa"), mb_ucfirst(""), mb_lcfirst("ABC"), mb_ucfirst("abc"));
