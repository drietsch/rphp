<?php
// IntlBreakIterator navigation: first/last/next/previous/following/
// preceding/isBoundary with ICU's cursor rules, the parts iterator, the
// code point iterator, the rules constructor, errors.
$b = IntlBreakIterator::createWordInstance('en');
var_dump($b->getText(), $b->current(), $b->first(), $b->last(), $b->next(), $b->previous(), $b->current());
var_dump($b->setText('Hello big world'));
echo 'next ', $b->next(), ' cur ', $b->current(), ' rs ', $b->getRuleStatus(), "\n";
echo 'next2 ', $b->next(2), ' cur ', $b->current(), "\n";
echo 'next-1 ', $b->next(-1), ' cur ', $b->current(), "\n";
echo 'next0 ', $b->next(0), ' next null ', $b->next(null), ' cur ', $b->current(), "\n";
echo 'last ', $b->last(), ' next ', $b->next(), ' cur ', $b->current(), ' next ', $b->next(), "\n";
echo 'first ', $b->first(), ' prev ', $b->previous(), ' cur ', $b->current(), ' next ', $b->next(), ' cur ', $b->current(), "\n";
foreach ([[0, 5], [0, 6], [0, 99], [2, 3], [5, -1], [5, -7], [3, -3], [3, -4]] as [$s, $n]) {
    $b->first();
    for ($i = 0; $i < $s; $i++) $b->next();
    echo 'from ', $b->current(), " next($n)=", $b->next($n), ' cur=', $b->current(), ' then next=', $b->next(), ' prev=', $b->previous(), "\n";
}
foreach ([-5, 0, 1, 3, 5, 6, 9, 14, 15, 16, 100] as $o) {
    echo "following($o)=", $b->following($o), ' cur ', $b->current(), " | preceding($o)=", $b->preceding($o), ' cur ', $b->current(), " | isBoundary($o)=", var_export($b->isBoundary($o), true), ' cur ', $b->current(), "\n";
}
$b->setText("é日本語x");
foreach ([0, 1, 2, 3, 4, 11, 12] as $o) {
    echo "isBoundary($o)=", var_export($b->isBoundary($o), true), ' cur ', $b->current(), ' following ', $b->following($o), ' preceding ', $b->preceding($o), "\n";
}
$p = $b->getPartsIterator();
var_dump(get_class($p), $p->getBreakIterator() === $b);
foreach ($p as $k => $v) echo "$k=[$v] rs=", $p->getRuleStatus(), "\n";
foreach ([IntlPartsIterator::KEY_LEFT, IntlPartsIterator::KEY_RIGHT] as $kt) {
    foreach ($b->getPartsIterator($kt) as $k => $v) echo "$k=[$v] ";
    echo "\n";
}
$s = IntlBreakIterator::createSentenceInstance('en');
$s->setText("One. Two? Three!\nFour");
foreach ($s->getPartsIterator() as $k => $v) echo "$k=", json_encode($v), ' ';
echo "\n";
$c = IntlBreakIterator::createCodePointInstance();
var_dump(get_class($c), $c->getLastCodePoint(), $c->getLocale(Locale::VALID_LOCALE));
$c->setText("aé😀");
foreach ($c as $k => $v) echo "$k=>$v lcp=", $c->getLastCodePoint(), "\n";
echo $c->previous(), ' ', $c->getLastCodePoint(), "\n";
echo $c->following(1), ' ', $c->getLastCodePoint(), ' ', $c->preceding(5), ' ', $c->getLastCodePoint(), "\n";
echo $c->next(2), ' ', $c->getLastCodePoint(), ' ', $c->next(5), ' ', $c->getLastCodePoint(), ' ', var_export($c->isBoundary(4), true), ' ', $c->current(), "\n";
$w = IntlBreakIterator::createWordInstance('en');
$r = new IntlRuleBasedBreakIterator($w->getRules());
$r->setText('ab cd');
echo implode(',', iterator_to_array($r)), ' ', $r->getRuleStatus(), ' [', $r->getLocale(Locale::VALID_LOCALE), "]\n";
$cl = clone $b;
var_dump($cl->current(), $cl->getText());
var_dump($b->getLocale(5), intl_get_error_code(), intl_get_error_message());
var_dump($b->getErrorCode(), $b->getErrorMessage());
try { $b->following(PHP_INT_MAX); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { $b->isBoundary(-PHP_INT_MAX); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { $b->getPartsIterator(7); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { new IntlBreakIterator(); } catch (Error $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
try { serialize($b); } catch (Exception $e) { echo $e->getMessage(), "\n"; }
