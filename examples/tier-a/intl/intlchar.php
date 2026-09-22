<?php
// Tier-A differential: IntlChar — code point in, property out: names and
// aliases, categories and the is* predicates, simple case mappings in the
// argument's shape, digits and numeric values, the enumerated properties
// by ICU's numbers, property names both ways, age and block.
$cps = ["A", "a", "ǅ", "1", "٣", "½", "Ⅻ", " ", "\u{00A0}", "\t", "\n", "\u{2028}", "_", "$", "-", "(", "é", "\u{0301}", "ß", "ǰ", "Σ", "ς", "İ", "ı", "\u{200B}", "\u{FEFF}", "\u{4E00}", "\u{AC00}", "\u{1F600}", "\u{1F1E6}", 0, 0x7F, 0x85, 0xAD, 0xE000, 0xFFFF, 0x110000, 0xD800, 0x3007, 0x1A2, 0x2F00];
foreach ($cps as $c) {
    $r = [];
    foreach (['ord', 'chr', 'charName', 'charType', 'charDirection', 'getCombiningClass', 'charDigitValue', 'getNumericValue', 'isalpha', 'isalnum', 'isdigit', 'isspace', 'isWhitespace', 'isUWhiteSpace', 'isJavaSpaceChar', 'islower', 'isupper', 'istitle', 'isULowercase', 'isUUppercase', 'isUAlphabetic', 'isxdigit', 'ispunct', 'isgraph', 'isprint', 'iscntrl', 'isISOControl', 'isblank', 'isbase', 'isdefined', 'isIDStart', 'isIDPart', 'isIDIgnorable', 'isJavaIDStart', 'isJavaIDPart', 'isMirrored', 'charMirror', 'getBidiPairedBracket', 'tolower', 'toupper', 'totitle', 'foldCase', 'getBlockCode', 'charAge'] as $m) {
        $r[$m] = IntlChar::$m($c);
    }
    echo json_encode($c), " ", json_encode($r, JSON_UNESCAPED_UNICODE), "\n";
}
var_dump(IntlChar::charName(0, IntlChar::EXTENDED_CHAR_NAME), IntlChar::charName(0xE000, IntlChar::EXTENDED_CHAR_NAME), IntlChar::charName(0xFFFF, IntlChar::EXTENDED_CHAR_NAME), IntlChar::charName(0x1A2, IntlChar::CHAR_NAME_ALIAS), IntlChar::charName(0xFEFF, IntlChar::CHAR_NAME_ALIAS), IntlChar::charName(0x41, IntlChar::UNICODE_10_CHAR_NAME), IntlChar::charName(0x17000), IntlChar::charName(0xF900), IntlChar::charName(0x2F800), IntlChar::charName(0x20000));
var_dump(IntlChar::charFromName("LATIN SMALL LETTER A"), IntlChar::charFromName("latin small letter a"), IntlChar::charFromName("NOPE"), intl_get_error_code(), IntlChar::charFromName("<control-0000>", IntlChar::EXTENDED_CHAR_NAME), IntlChar::charFromName("CJK UNIFIED IDEOGRAPH-4E00"), IntlChar::charFromName("LATIN CAPITAL LETTER GHA", IntlChar::CHAR_NAME_ALIAS), IntlChar::charFromName("HANGUL SYLLABLE GA"), IntlChar::charFromName("GRINNING FACE"));
var_dump(IntlChar::digit("7"), IntlChar::digit("a", 16), IntlChar::digit("z", 36), IntlChar::digit("z", 16), IntlChar::digit("٣"), IntlChar::digit("Ａ", 16), IntlChar::digit("x", 1), IntlChar::forDigit(7), IntlChar::forDigit(11, 16), IntlChar::forDigit(11, 10), IntlChar::forDigit(35, 36), IntlChar::forDigit(-1));
var_dump(IntlChar::getUnicodeVersion(), IntlChar::hasBinaryProperty("A", IntlChar::PROPERTY_ALPHABETIC), IntlChar::hasBinaryProperty("1", IntlChar::PROPERTY_ALPHABETIC), IntlChar::hasBinaryProperty("-", IntlChar::PROPERTY_DASH), IntlChar::hasBinaryProperty("\u{1F600}", 57), IntlChar::hasBinaryProperty("\u{1F600}", 58), IntlChar::hasBinaryProperty("#", 61), IntlChar::hasBinaryProperty("A", IntlChar::PROPERTY_CASED), IntlChar::hasBinaryProperty("A", 9999), IntlChar::hasBinaryProperty("é", IntlChar::PROPERTY_NFD_INERT), IntlChar::hasBinaryProperty("a", IntlChar::PROPERTY_NFD_INERT));
foreach (["A", "é", "\u{0301}", "٣", "中", "あ", "\u{0628}", "\u{0627}", "(", ")", "\u{1F600}", "\u{AC00}", "\u{1100}", "\u{0915}", "\u{094D}", "。", "\u{3000}", "x", "﹁"] as $c) {
    $r = [];
    foreach (['BIDI_CLASS', 'BLOCK', 'CANONICAL_COMBINING_CLASS', 'EAST_ASIAN_WIDTH', 'GENERAL_CATEGORY', 'JOINING_GROUP', 'JOINING_TYPE', 'LINE_BREAK', 'NUMERIC_TYPE', 'SCRIPT', 'HANGUL_SYLLABLE_TYPE', 'NFD_QUICK_CHECK', 'NFC_QUICK_CHECK', 'LEAD_CANONICAL_COMBINING_CLASS', 'TRAIL_CANONICAL_COMBINING_CLASS', 'GRAPHEME_CLUSTER_BREAK', 'SENTENCE_BREAK', 'WORD_BREAK', 'BIDI_PAIRED_BRACKET_TYPE', 'GENERAL_CATEGORY_MASK', 'ALPHABETIC'] as $p) {
        $r[$p] = IntlChar::getIntPropertyValue($c, constant("IntlChar::PROPERTY_$p"));
    }
    foreach ([0x1017 => "INDIC_SYLLABIC_CATEGORY", 0x1018 => "VERTICAL_ORIENTATION"] as $pv => $p) {
        $r[$p] = IntlChar::getIntPropertyValue($c, $pv);
    }
    echo json_encode($c), " ", json_encode($r), "\n";
}
foreach ([IntlChar::PROPERTY_GENERAL_CATEGORY, IntlChar::PROPERTY_SCRIPT, IntlChar::PROPERTY_BIDI_CLASS, IntlChar::PROPERTY_ALPHABETIC, IntlChar::PROPERTY_BLOCK, IntlChar::PROPERTY_LINE_BREAK, IntlChar::PROPERTY_NUMERIC_VALUE, IntlChar::PROPERTY_AGE, IntlChar::PROPERTY_NAME, IntlChar::PROPERTY_SCRIPT_EXTENSIONS, IntlChar::PROPERTY_GENERAL_CATEGORY_MASK, 57, 9999] as $p) {
    echo json_encode([$p, IntlChar::getPropertyName($p, IntlChar::SHORT_PROPERTY_NAME), IntlChar::getPropertyName($p, IntlChar::LONG_PROPERTY_NAME), IntlChar::getPropertyName($p, 5), IntlChar::getIntPropertyMinValue($p), IntlChar::getIntPropertyMaxValue($p)]), "\n";
}
foreach ([[IntlChar::PROPERTY_GENERAL_CATEGORY, 1], [IntlChar::PROPERTY_GENERAL_CATEGORY, 29], [IntlChar::PROPERTY_GENERAL_CATEGORY, 30], [IntlChar::PROPERTY_SCRIPT, 25], [IntlChar::PROPERTY_SCRIPT, 0], [IntlChar::PROPERTY_BIDI_CLASS, 1], [IntlChar::PROPERTY_BLOCK, 1], [IntlChar::PROPERTY_LINE_BREAK, 5], [IntlChar::PROPERTY_ALPHABETIC, 1], [IntlChar::PROPERTY_ALPHABETIC, 0], [IntlChar::PROPERTY_EAST_ASIAN_WIDTH, 3], [IntlChar::PROPERTY_JOINING_GROUP, 2], [IntlChar::PROPERTY_CANONICAL_COMBINING_CLASS, 230], [IntlChar::PROPERTY_HANGUL_SYLLABLE_TYPE, 1]] as [$p, $v]) {
    echo json_encode([$p, $v, IntlChar::getPropertyValueName($p, $v, IntlChar::SHORT_PROPERTY_NAME), IntlChar::getPropertyValueName($p, $v, IntlChar::LONG_PROPERTY_NAME), IntlChar::getPropertyValueName($p, $v, 9)]), "\n";
}
var_dump(IntlChar::getPropertyEnum("Alphabetic"), IntlChar::getPropertyEnum("alpha"), IntlChar::getPropertyEnum("General_Category"), IntlChar::getPropertyEnum("general category"), IntlChar::getPropertyEnum("gc"), IntlChar::getPropertyEnum("Script"), IntlChar::getPropertyEnum("nope"), IntlChar::getPropertyValueEnum(IntlChar::PROPERTY_GENERAL_CATEGORY, "Lu"), IntlChar::getPropertyValueEnum(IntlChar::PROPERTY_GENERAL_CATEGORY, "uppercase letter"), IntlChar::getPropertyValueEnum(IntlChar::PROPERTY_SCRIPT, "Latn"), IntlChar::getPropertyValueEnum(IntlChar::PROPERTY_SCRIPT, "latin"), IntlChar::getPropertyValueEnum(IntlChar::PROPERTY_BLOCK, "Basic Latin"), IntlChar::getPropertyValueEnum(IntlChar::PROPERTY_ALPHABETIC, "Yes"), IntlChar::getPropertyValueEnum(IntlChar::PROPERTY_ALPHABETIC, "maybe"), IntlChar::getPropertyValueEnum(IntlChar::PROPERTY_SCRIPT, "nope"));
$names = []; IntlChar::enumCharNames(0x41, 0x46, function ($cp, $choice, $name) use (&$names) { $names[] = "$cp:$choice:$name"; }); var_dump($names);
$names = []; IntlChar::enumCharNames("a", "d", function ($cp, $choice, $name) use (&$names) { $names[] = "$cp:$choice:$name"; }, IntlChar::EXTENDED_CHAR_NAME); var_dump($names);
$types = []; IntlChar::enumCharTypes(function ($start, $end, $type) use (&$types) { if ($start < 0x100) $types[] = "$start-$end:$type"; }); var_dump(count($types) > 20, array_slice($types, 0, 12));
var_dump(IntlChar::ord("ab"), IntlChar::chr(-1), IntlChar::charName("ab"), IntlChar::tolower("ab"), IntlChar::getFC_NFKC_Closure("A"), IntlChar::getFC_NFKC_Closure(0x110000));
try { IntlChar::charName("A", 9); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
