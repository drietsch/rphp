<?php
// mb_strtoupper / mb_strtolower / mb_convert_case in all eight modes:
// full vs simple mappings, SpecialCasing expansions, final sigma, title-case
// word boundaries, Turkish dotted/dotless i under ISO-8859-9, invalid bytes.
$samples = [
    "hello world", "HELLO WORLD", "straße", "STRASSE", "ǆ ǅ Ǆ", "ŉ ﬁ ﬀ", "İstanbul ı i I",
    "Σίσυφος ΣΊΣΥΦΟΣ ΟΔΟΣ Σ.", "ΣΑΣ σασ", "αβγ ΑΒΓ", "привет МИР", "o'neil mc'donald l'été",
    "hello-world hello_world hello.world", "123abc ABC123", "ᾈ ᾀ", "ǈ ǉ Ǉ", "Ⅷ ⅷ", "ⓐ Ⓐ", "𐐀 𐐨",
    "ﬀﬁﬂﬃﬄﬅﬆ", "ῼ ῳ ῴ", "\u{1E9E} \u{DF}", "café CAFÉ Café", "éàü ÉÀÜ", "ǰ ẖ ẗ ẘ ẙ ẚ", "𝐀 𝐚", "ａｂｃ ＡＢＣ",
    "a\xffb", "",
];
$modes = [
    "UPPER" => MB_CASE_UPPER, "LOWER" => MB_CASE_LOWER, "TITLE" => MB_CASE_TITLE, "FOLD" => MB_CASE_FOLD,
    "UPPER_SIMPLE" => MB_CASE_UPPER_SIMPLE, "LOWER_SIMPLE" => MB_CASE_LOWER_SIMPLE,
    "TITLE_SIMPLE" => MB_CASE_TITLE_SIMPLE, "FOLD_SIMPLE" => MB_CASE_FOLD_SIMPLE,
];
foreach ($samples as $s) {
    echo json_encode($s), "\n";
    foreach ($modes as $name => $mode) {
        echo "  ", str_pad($name, 12), " ", json_encode(mb_convert_case($s, $mode)), "\n";
    }
    echo "  upper/lower  ", json_encode(mb_strtoupper($s)), " ", json_encode(mb_strtolower($s)), "\n";
}

echo "-- Turkish (ISO-8859-9)\n";
var_dump(bin2hex(mb_strtoupper("i", "ISO-8859-9")), bin2hex(mb_strtolower("I", "ISO-8859-9")), bin2hex(mb_strtolower("\xdd", "ISO-8859-9")), bin2hex(mb_convert_case("I", MB_CASE_FOLD, "ISO-8859-9")));
var_dump(mb_strtoupper("i", "ISO-8859-1"), mb_strtolower("I"));

echo "-- other encodings\n";
var_dump(bin2hex(mb_strtoupper("\x00\x61\xd8\x01\xdc\x28", "UTF-16BE")), mb_strtoupper("\x82\xa0abc", "SJIS") === "\x82\xa0ABC", bin2hex(mb_strtolower("\xc0\xc9", "ISO-8859-1")), mb_strtoupper("caf\xe9", "ISO-8859-1") === "CAF\xc9");
var_dump(mb_convert_case("abc", 8));
