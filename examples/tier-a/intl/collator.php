<?php
// Tier-A differential: Collator — compare, sort keys, the three sort modes,
// attributes and strength, locale resolution, per-object errors.
$c = new Collator('en_US');
var_dump($c->compare("a", "b"), $c->compare("b", "a"), $c->compare("a", "a"), $c->compare("a", "A"), $c->compare("A", "a"), $c->compare("ä", "b"), $c->compare("resume", "résumé"), $c->compare("co-op", "coop"), $c->compare("10", "9"), $c->compare("\xff", "a"), $c->getErrorMessage(), $c->getErrorCode(), intl_get_error_message());
var_dump(bin2hex($c->getSortKey("Hello")), bin2hex($c->getSortKey("héllo")), bin2hex($c->getSortKey("")), bin2hex($c->getSortKey("ABC")), bin2hex($c->getSortKey("abc")), $c->getSortKey("\xff"));
$a = ["b", "a", "10", "9", "A", "ä", "1.5", "z", "Z", "ẑ"]; var_dump($c->sort($a), $a);
$a = ["b", "a", "10", "9", 2, 1.5, "x" => "c"]; $c->sort($a, Collator::SORT_NUMERIC); var_dump($a);
$a = ["b", "a", "10", "9", "A", "ä", "x" => "c"]; $c->sort($a, Collator::SORT_STRING); var_dump($a);
$a = ["b", "a", "10", "9", "A", "ä", 2, "x" => "c", null, true]; $c->asort($a); var_dump($a);
$a = ["b", "a", "10", "9", "A", "ä", "x" => "c", 3]; var_dump($c->sortWithSortKeys($a), $a);
$a = ["b", ["a"]]; var_dump($c->sort($a), $a);
var_dump($c->getStrength(), $c->getAttribute(Collator::STRENGTH), $c->getAttribute(Collator::CASE_FIRST), $c->getAttribute(Collator::NUMERIC_COLLATION), $c->getAttribute(Collator::ALTERNATE_HANDLING));
var_dump($c->setAttribute(Collator::NUMERIC_COLLATION, Collator::ON), $c->compare("10", "9"), $c->compare("a10", "a9"), $c->setStrength(Collator::PRIMARY), $c->compare("a", "A"), $c->compare("a", "á"), $c->getStrength(), $c->setStrength(Collator::SECONDARY), $c->compare("a", "A"), $c->compare("a", "á"), $c->setAttribute(Collator::CASE_FIRST, Collator::UPPER_FIRST), $c->setStrength(Collator::TERTIARY), $c->compare("a", "A"), $c->setAttribute(Collator::CASE_FIRST, Collator::LOWER_FIRST), $c->compare("a", "A"), $c->setAttribute(Collator::ALTERNATE_HANDLING, Collator::SHIFTED), $c->compare("co-op", "coop"), $c->compare("co op", "coop"));
var_dump($c->setAttribute(99, 1), $c->getErrorMessage(), intl_get_error_message(), $c->getAttribute(99), $c->getErrorMessage(), $c->setAttribute(Collator::STRENGTH, 99), $c->getErrorMessage(), $c->getLocale(99), $c->getErrorMessage());
foreach (['en_US', 'en', 'de_DE', 'de_AT', 'fr_CA', 'fr', 'es', 'sv_SE', 'zh', 'root', '', 'xx', 'de_DE@collation=phonebook', 'de-u-co-phonebk', 'pt_BR', 'da', 'nb_NO', 'th', 'el'] as $l) {
    $x = collator_create($l);
    echo "[$l] valid=", collator_get_locale($x, Locale::VALID_LOCALE), " actual=", collator_get_locale($x, Locale::ACTUAL_LOCALE), " attrs=", json_encode([collator_get_attribute($x, Collator::FRENCH_COLLATION), collator_get_attribute($x, Collator::ALTERNATE_HANDLING), collator_get_attribute($x, Collator::CASE_FIRST), collator_get_attribute($x, Collator::NORMALIZATION_MODE), collator_get_strength($x)]), "\n";
}
$de = new Collator('de_DE'); $ph = new Collator('de_DE@collation=phonebook'); $sv = new Collator('sv_SE'); $cs = new Collator('cs_CZ');
$words = ["Müller", "Mueller", "Mull", "Mulle", "Zebra", "Ähre", "Apfel", "Ärger", "Öl", "Ozean", "Übel", "Uber", "chalupa", "cukr", "hrad", "chata", "Åke", "Zach", "Ärlig"];
foreach ([$de, $ph, $sv, $cs] as $col) { $w = $words; $col->sort($w); echo implode(",", $w), "\n"; }
var_dump(collator_compare($de, "a", "b"), collator_get_error_code($de), collator_get_error_message($de));
try { $k = clone $c; } catch (Error $e) { echo $e->getMessage(), "\n"; }
