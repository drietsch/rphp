<?php
// Tier-A differential: the grapheme_* functions — cluster-based length,
// search, substrings, extraction and splitting as php's ext/intl answers
// (an ASCII string takes the byte path, offsets count clusters, a negative
// start walks from the end).
$s = "a\u{0301}b\u{0301}c\u{0301}de"; // 5 clusters: á b́ ć d e
$flag = "\u{1F1E9}\u{1F1EA}"; // 🇩🇪 one cluster
$z = "z\u{200D}\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}"; // z + family emoji
var_dump(grapheme_strlen("abc"), grapheme_strlen(""), grapheme_strlen($s), grapheme_strlen($flag . $z), grapheme_strlen("\xff"), intl_get_error_message(), grapheme_strlen("e\u{0301}\u{0301}"));
foreach ([[$s, "b\u{0301}"], [$s, "c"], [$s, "c\u{0301}", 1], [$s, "d", -3], [$s, "d", -1], ["abcabc", "c", 3], ["abcabc", "c", -1], ["abcabc", "x"], [$s, ""], ["abc", "", 1]] as $case) {
    [$h, $n] = $case; $o = $case[2] ?? 0;
    echo json_encode([grapheme_strpos($h, $n, $o), grapheme_stripos($h, $n, $o), grapheme_strrpos($h, $n, $o), grapheme_strripos($h, $n, $o)]), "\n";
}
var_dump(grapheme_stripos("ÁBC", "áb"), grapheme_strripos("ÁBCábc", "áb"), grapheme_strpos("abc", "b", 3), grapheme_strpos("abc", "b", -3));
foreach ([[$s, 0], [$s, 1], [$s, -1], [$s, -2, 1], [$s, 1, 2], [$s, 1, -1], [$s, 0, 0], [$s, 5], [$s, 6], [$s, -6], [$s, 2, -4], [$s, -5, 3], [$s, 3, 10], ["abcdef", 2], ["abcdef", -2], ["abcdef", 2, -1], ["abcdef", 7], ["abcdef", -7], ["abcdef", 2, 0], ["abcdef", 0, -7], [$flag . $z, 1], [$flag . $z, 0, 1], [$flag . $z, -1]] as $case) {
    $r = isset($case[2]) ? grapheme_substr($case[0], $case[1], $case[2]) : grapheme_substr($case[0], $case[1]);
    echo json_encode($case), " => ", var_export($r, true), " ", intl_get_error_code(), "\n";
}
var_dump(grapheme_substr("\xff", 0));
foreach ([[$s, "c\u{0301}"], [$s, "c\u{0301}", true], [$s, "x"], ["abc", "b"], ["abc", "b", true], ["ÁBC", "b"], [$s, ""]] as $case) {
    $r = grapheme_strstr($case[0], $case[1], $case[2] ?? false); $r2 = grapheme_stristr($case[0], $case[1], $case[2] ?? false);
    echo json_encode([$r, $r2]), "\n";
}
foreach ([[$s, 2], [$s, 2, GRAPHEME_EXTR_MAXBYTES], [$s, 4, GRAPHEME_EXTR_MAXBYTES], [$s, 2, GRAPHEME_EXTR_MAXCHARS], [$s, 3, GRAPHEME_EXTR_MAXCHARS], [$s, 10], [$s, 0], [$s, 2, GRAPHEME_EXTR_COUNT, 3], [$s, 2, GRAPHEME_EXTR_COUNT, 1], ["abcdef", 3], ["abcdef", 3, GRAPHEME_EXTR_COUNT, 4], ["abcdef", 3, GRAPHEME_EXTR_COUNT, 6], ["abcdef", 3, GRAPHEME_EXTR_COUNT, -2], [$flag . $z, 1], [$flag . $z, 5, GRAPHEME_EXTR_MAXBYTES]] as $case) {
    $next = null;
    $r = grapheme_extract($case[0], $case[1], $case[2] ?? GRAPHEME_EXTR_COUNT, $case[3] ?? 0, $next);
    echo json_encode([$r, $next]), " ", intl_get_error_code(), "\n";
}
var_dump(grapheme_str_split($s), grapheme_str_split($s, 2), grapheme_str_split($s, 3), grapheme_str_split(""), grapheme_str_split("abc", 5), grapheme_str_split($flag . $z, 2));
var_dump(grapheme_levenshtein("kitten", "sitting"), grapheme_levenshtein($s, "abcde"), grapheme_levenshtein("", "abc"), grapheme_levenshtein("a\u{0301}", "a"), grapheme_levenshtein("abc", "abd", 2, 3, 4), grapheme_levenshtein("ab", "abc", 2, 3, 4), grapheme_levenshtein("abc", "ab", 2, 3, 4));
try { grapheme_extract("abc", -1); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { grapheme_extract("abc", 1, 9); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { grapheme_str_split("abc", 0); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { grapheme_strpos("abc", "b", -5); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
