<?php
// Transliterator: invalid IDs, rules, ranges and arguments.
foreach (['bogus', ';Any-Latin', 'Any-Latin;;Latin-ASCII', '::Any-Latin', 'Any-Latin/BGN', 'Latin-Any', 'x', 'Upper;[A-Z]'] as $id) {
    var_dump(Transliterator::create($id));
    var_dump(intl_get_error_code(), intl_get_error_message());
}
var_dump(transliterator_create('Han-Latin', Transliterator::REVERSE), intl_get_error_message());
var_dump(transliterator_transliterate('bogus', 'abc'));
var_dump(intl_get_error_code(), intl_get_error_message());
$t = Transliterator::create('Any-Upper');
var_dump($t->transliterate('abc', 5));
var_dump($t->getErrorCode(), $t->getErrorMessage(), intl_get_error_code());
var_dump($t->transliterate("a\xffb"));
var_dump($t->getErrorCode(), $t->getErrorMessage());
var_dump($t->transliterate('abc'), $t->getErrorCode(), $t->getErrorMessage());
foreach ([[-1, -1], [0, -2], [2, 1]] as [$s, $e]) {
    try { $t->transliterate('abc', $s, $e); } catch (ValueError $x) { echo $x->getMessage(), "\n"; }
}
try { transliterator_transliterate($t, 'abc', 2, 1); } catch (ValueError $x) { echo $x->getMessage(), "\n"; }
try { Transliterator::create('Any-Upper', 5); } catch (ValueError $x) { echo $x->getMessage(), "\n"; }
try { transliterator_create_from_rules('a > b;', 5); } catch (ValueError $x) { echo $x->getMessage(), "\n"; }
try { new Transliterator(); } catch (Error $x) { echo get_class($x), ': ', $x->getMessage(), "\n"; }
try { transliterator_create_inverse('x'); } catch (TypeError $x) { echo $x->getMessage(), "\n"; }
try { $t->id = 'x'; } catch (Error $x) { echo $x->getMessage(), "\n"; }
try { serialize($t); } catch (Exception $x) { echo get_class($x), ': ', $x->getMessage(), "\n"; }
// rules
$r = Transliterator::createFromRules('a > b; c > d;');
var_dump($r->id, $r->transliterate('abcabc'), $r->createInverse(), intl_get_error_message());
$r = Transliterator::createFromRules('a <> b; c <> d;', Transliterator::REVERSE);
var_dump($r->transliterate('abcd'));
var_dump(Transliterator::createFromRules('a > b')->transliterate('aaa'));
var_dump(Transliterator::createFromRules(':: Any-Upper; x > y;')->transliterate('xax'));
var_dump(Transliterator::createFromRules(':: Latin-ASCII; :: Lower;')->transliterate('ÄÖÜ Straße'));
var_dump(Transliterator::createFromRules('a > ;; [ > b'));
var_dump(intl_get_error_code(), intl_get_error_message());
var_dump(Transliterator::createFromRules('$x = a; $y > b;'));
var_dump(intl_get_error_code(), intl_get_error_message());
