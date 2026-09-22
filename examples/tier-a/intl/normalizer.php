<?php
// Tier-A differential: Normalizer — the five forms, isNormalized, the raw
// decomposition of one code point, and the argument errors.
$forms = ['FORM_D' => Normalizer::FORM_D, 'FORM_KD' => Normalizer::FORM_KD, 'FORM_C' => Normalizer::FORM_C, 'FORM_KC' => Normalizer::FORM_KC, 'FORM_KC_CF' => Normalizer::FORM_KC_CF];
$inputs = ["Å", "A\u{030A}", "\u{212B}", "ﬁ", "①", "Ｈｅｌｌｏ", "e\u{0301}\u{0327}", "가", "\u{1100}\u{1161}", "ABC def", "ß", "\u{00AD}soft", "", "abc\u{0301}", "Ⅻ", "㍿", "ṩ", "\u{1E9B}\u{0323}", "\u{FB01}\u{0301}", "Ⓐ"];
foreach ($inputs as $in) {
    echo json_encode($in), ":";
    foreach ($forms as $name => $f) { echo " $name=", json_encode(normalizer_normalize($in, $f)), "/", var_export(normalizer_is_normalized($in, $f), true); }
    echo "\n";
}
var_dump(Normalizer::normalize("abc"), Normalizer::isNormalized("abc"), Normalizer::normalize("\xff"), intl_get_error_message(), intl_get_error_code(), Normalizer::isNormalized("\xff"), intl_get_error_message());
foreach (["\u{FB01}", "a", "ab", "\u{212B}", "\u{AC00}", "\u{00C5}", "\u{1E9B}", "\u{2126}", "", "\u{0344}", "\u{FDFA}"] as $c) {
    echo json_encode($c), ": ";
    foreach ($forms as $name => $f) { echo " $name=", json_encode(Normalizer::getRawDecomposition($c, $f)); }
    echo " err=", intl_get_error_code(), "\n";
}
var_dump(Normalizer::getRawDecomposition("\xff"), intl_get_error_message());
try { normalizer_normalize("abc", 99); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { Normalizer::isNormalized("abc", 3); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
var_dump(Normalizer::getRawDecomposition("\u{2126}", 99));
