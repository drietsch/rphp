<?php
// NumberFormatter, part two: toPattern rules, padding, parsing (strict and lenient), attribute edge cases, symbols, currency resolution.

function t_1($cb) { $f = new NumberFormatter("en_US", NumberFormatter::DECIMAL); $cb($f); echo json_encode($f->getPattern()), " np=", json_encode($f->getTextAttribute(NumberFormatter::NEGATIVE_PREFIX)), " pp=", json_encode($f->getTextAttribute(NumberFormatter::POSITIVE_PREFIX)), " ", $f->format(-5), "\n"; }
t_1(fn($f) => $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "pre"));
t_1(fn($f) => $f->setTextAttribute(NumberFormatter::POSITIVE_SUFFIX, "x"));
t_1(fn($f) => $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, "-"));
t_1(fn($f) => $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, "neg"));
t_1(fn($f) => $f->setTextAttribute(NumberFormatter::NEGATIVE_SUFFIX, ""));
t_1(fn($f) => $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, ""));
t_1(fn($f) => $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "a'b%c"));
t_1(fn($f) => $f->setTextAttribute(NumberFormatter::PADDING_CHARACTER, "*"));
t_1(fn($f) => $f->setPattern("#,##0.###;(#)"));
t_1(function($f) { $f->setPattern("#,##0.###;(#)"); $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "p"); });
t_1(function($f) { $f->setPattern("#,##0.###;(#)"); $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, "-"); $f->setTextAttribute(NumberFormatter::NEGATIVE_SUFFIX, ""); });
t_1(function($f) { $f->setPattern("#,##0.###;-#"); });
t_1(function($f) { $f->setPattern("#,##0.###;-#,##0.00"); });
t_1(function($f) { $f->setPattern("#,##0.###;'-'#"); });
t_1(function($f) { $f->setPattern("#,##0.###;#"); });
t_1(function($f) { $f->setPattern("#,##0.###;#-"); });
// padding
t_1(function($f) { $f->setPattern("*x##0.0"); });
t_1(function($f) { $f->setPattern("*x#,##0.0"); });
t_1(function($f) { $f->setPattern("'ab'*x#,##0.0"); });
t_1(function($f) { $f->setPattern("#,##0.0*x"); });
t_1(function($f) { $f->setPattern("#,##0.0'ab'*x"); });
t_1(function($f) { $f->setPattern("*x#"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setPattern("*x##,##0.0"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setPattern("*x#,##0.0;(#)"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setPattern("*x0.0E00"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setPattern("*x@@#"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setPattern("*x0.25"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); $f->setAttribute(NumberFormatter::PADDING_POSITION, NumberFormatter::PAD_BEFORE_SUFFIX); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); $f->setAttribute(NumberFormatter::PADDING_POSITION, NumberFormatter::PAD_AFTER_SUFFIX); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); $f->setAttribute(NumberFormatter::PADDING_POSITION, NumberFormatter::PAD_AFTER_PREFIX); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); $f->setAttribute(NumberFormatter::PADDING_POSITION, NumberFormatter::PAD_BEFORE_PREFIX); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); $f->setTextAttribute(NumberFormatter::PADDING_CHARACTER, "'"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); $f->setTextAttribute(NumberFormatter::PADDING_CHARACTER, "*"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); $f->setTextAttribute(NumberFormatter::PADDING_CHARACTER, "ab"); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });
t_1(function($f) { $f->setAttribute(NumberFormatter::FORMAT_WIDTH, 8); $f->setTextAttribute(NumberFormatter::PADDING_CHARACTER, ""); echo $f->getAttribute(NumberFormatter::FORMAT_WIDTH), " "; });


function t_2($cb) { $f = new NumberFormatter("en_US", NumberFormatter::DECIMAL); $cb($f); echo json_encode($f->getPattern()), " np=", json_encode($f->getTextAttribute(NumberFormatter::NEGATIVE_PREFIX)), " ns=", json_encode($f->getTextAttribute(NumberFormatter::NEGATIVE_SUFFIX)), " pp=", json_encode($f->getTextAttribute(NumberFormatter::POSITIVE_PREFIX)), " ps=", json_encode($f->getTextAttribute(NumberFormatter::POSITIVE_SUFFIX)), " ", $f->format(-5), " ", $f->format(5), "\n"; }
t_2(function($f) { $f->setPattern("#,##0.###;-#"); $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "p"); });
t_2(function($f) { $f->setPattern("#,##0.###;-#"); $f->setTextAttribute(NumberFormatter::POSITIVE_SUFFIX, "p"); });
t_2(function($f) { $f->setPattern("#,##0.###;-#,##0.00"); $f->setTextAttribute(NumberFormatter::POSITIVE_SUFFIX, "p"); });
t_2(function($f) { $f->setPattern("'x'#,##0.###"); });
t_2(function($f) { $f->setPattern("'x'#,##0.###;-#"); });
t_2(function($f) { $f->setPattern("'x'#,##0.###;-'x'#"); });
t_2(function($f) { $f->setPattern("'x'#,##0.###;'x'-#"); });
t_2(function($f) { $f->setPattern("#,##0.###'x';-#'x'"); });
t_2(function($f) { $f->setPattern("#,##0.###'x';-#"); });
t_2(function($f) { $f->setPattern("#,##0.###'x'"); });
t_2(function($f) { $f->setPattern("¤#,##0.00"); });
t_2(function($f) { $f->setPattern("¤#,##0.00;-¤#"); });
t_2(function($f) { $f->setPattern("¤#,##0.00;(¤#)"); });
t_2(function($f) { $f->setPattern("#,##0.00;-#,##0.00"); $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, "-"); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "p"); $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, ""); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, "n"); $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, ""); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, "n"); $f->setPattern("#,##0"); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "p"); $f->setPattern("#,##0"); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "p"); $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, "-"); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "p"); $f->setTextAttribute(NumberFormatter::NEGATIVE_PREFIX, "-p"); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "-"); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "¤"); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "%"); });
t_2(function($f) { $f->setTextAttribute(NumberFormatter::POSITIVE_PREFIX, "a-b"); });


function t_3($loc, $style, $pat, $cb = null) { $f = new NumberFormatter($loc, $style, $pat); if ($cb) $cb($f); echo json_encode($f->getPattern()), " cc=", json_encode($f->getTextAttribute(NumberFormatter::CURRENCY_CODE)), " sym=", json_encode($f->getSymbol(NumberFormatter::CURRENCY_SYMBOL)), "/", json_encode($f->getSymbol(NumberFormatter::INTL_CURRENCY_SYMBOL)), " ", $f->format(-5), " ", $f->format(5), " fc=", $f->formatCurrency(5, "EUR"), " ", $f->formatCurrency(5, "JPY"), " frac=", $f->getAttribute(NumberFormatter::MIN_FRACTION_DIGITS), "/", $f->getAttribute(NumberFormatter::MAX_FRACTION_DIGITS), "\n"; }
t_3("en_US", NumberFormatter::DECIMAL, "");
t_3("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.00 ¤");
t_3("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.00 ¤¤");
t_3("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.00 ¤¤¤");
t_3("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.### ¤");
t_3("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0 ¤");
t_3("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.0 ¤");
t_3("en_US", NumberFormatter::DECIMAL, "", fn($f) => $f->setPattern("#,##0.00 ¤"));
t_3("en_US", NumberFormatter::DECIMAL, "", fn($f) => $f->setPattern("#,##0.### ¤"));
t_3("en_US", NumberFormatter::DECIMAL, "", fn($f) => $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "EUR"));
t_3("en_US", NumberFormatter::DECIMAL, "", fn($f) => $f->setTextAttribute(NumberFormatter::POSITIVE_SUFFIX, "¤"));
t_3("de_DE", NumberFormatter::DECIMAL, "", fn($f) => $f->setSymbol(NumberFormatter::CURRENCY_SYMBOL, "X"));
t_3("de_DE", NumberFormatter::PATTERN_DECIMAL, "#,##0.00 ¤", fn($f) => $f->setSymbol(NumberFormatter::CURRENCY_SYMBOL, "X"));
t_3("de_DE", NumberFormatter::PATTERN_DECIMAL, "#,##0.00 ¤", fn($f) => $f->setSymbol(NumberFormatter::INTL_CURRENCY_SYMBOL, "USD"));
t_3("de_DE", NumberFormatter::PATTERN_DECIMAL, "#,##0.00 ¤¤", fn($f) => $f->setSymbol(NumberFormatter::INTL_CURRENCY_SYMBOL, "USD"));
t_3("de_DE", NumberFormatter::PATTERN_DECIMAL, "#,##0.00 ¤", fn($f) => $f->setSymbol(NumberFormatter::INTL_CURRENCY_SYMBOL, "JPY"));
t_3("de_DE", NumberFormatter::CURRENCY, "", fn($f) => $f->setSymbol(NumberFormatter::INTL_CURRENCY_SYMBOL, "JPY"));
t_3("de_DE", NumberFormatter::CURRENCY, "", fn($f) => $f->setSymbol(NumberFormatter::CURRENCY_SYMBOL, "X"));
t_3("de_DE", NumberFormatter::CURRENCY, "", fn($f) => $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "JPY"));
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "JPY"); $f->setSymbol(NumberFormatter::CURRENCY_SYMBOL, "X"); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setSymbol(NumberFormatter::CURRENCY_SYMBOL, "X"); $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "JPY"); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "JPY"); $f->setSymbol(NumberFormatter::INTL_CURRENCY_SYMBOL, "USD"); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setSymbol(NumberFormatter::INTL_CURRENCY_SYMBOL, "USD"); $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "JPY"); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "jpy"); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "ZZZ"); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "ZZ"); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, ""); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setAttribute(NumberFormatter::FRACTION_DIGITS, 1); $f->setTextAttribute(NumberFormatter::CURRENCY_CODE, "JPY"); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setAttribute(NumberFormatter::MAX_FRACTION_DIGITS, 1); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setAttribute(NumberFormatter::MIN_FRACTION_DIGITS, 1); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setAttribute(NumberFormatter::MIN_FRACTION_DIGITS, 3); });
t_3("de_DE", NumberFormatter::CURRENCY, "", function($f) { $f->setAttribute(NumberFormatter::MAX_FRACTION_DIGITS, 3); });
t_3("en_US", NumberFormatter::DECIMAL, "", function($f) { $f->setAttribute(NumberFormatter::MAX_FRACTION_DIGITS, 1); });
t_3("en_US", NumberFormatter::DECIMAL, "", function($f) { $f->setAttribute(NumberFormatter::MIN_FRACTION_DIGITS, 5); });
t_3("en_US", NumberFormatter::DECIMAL, "", function($f) { $f->setAttribute(NumberFormatter::MAX_INTEGER_DIGITS, 2); });
t_3("en_US", NumberFormatter::DECIMAL, "", function($f) { $f->setAttribute(NumberFormatter::MIN_INTEGER_DIGITS, 0); });
t_3("en_US", NumberFormatter::DECIMAL, "", function($f) { $f->setAttribute(NumberFormatter::MIN_INTEGER_DIGITS, 0); $f->setAttribute(NumberFormatter::MAX_FRACTION_DIGITS, 0); });


$f = new NumberFormatter("en_US", NumberFormatter::DECIMAL);
$g = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.###;(#)");
$c = new NumberFormatter("en_US", NumberFormatter::CURRENCY);
$d = new NumberFormatter("de_DE", NumberFormatter::DECIMAL);
$n = new NumberFormatter("en_US", NumberFormatter::DECIMAL); $n->setAttribute(NumberFormatter::GROUPING_USED, 0);
$l = new NumberFormatter("en_US", NumberFormatter::DECIMAL); $l->setAttribute(NumberFormatter::LENIENT_PARSE, 1);
$pc = new NumberFormatter("en_US", NumberFormatter::PERCENT);
$ac = new NumberFormatter("en_US", NumberFormatter::CURRENCY_ACCOUNTING);
$sc = new NumberFormatter("en_US", NumberFormatter::SCIENTIFIC);
function p_4($f, $s) { $p = 0; $r = $f->parse($s, NumberFormatter::TYPE_DOUBLE, $p); echo json_encode($s), " => ", var_export($r, true), " @", $p, "\n"; }
foreach (["(42", "42)", "(42)", "42", "-42", "(-42)", "1,234", "1,234.5"] as $s) p_4($g, $s);
echo "--currency\n";
foreach (["1234.5", "$1234.5 x", "$ 1,234.5", "-$1,234.5", "$-1,234.5", "USD1,234.5", "USD 1,234.5", "1,234.5$", "€5", "¤5"] as $s) p_4($c, $s);
echo "--accounting\n";
foreach (["($1,234.5)", "-$1,234.5", "$1,234.5", "(1,234.5)"] as $s) p_4($ac, $s);
echo "--decimal\n";
foreach (["1.234,5", "1,234,56", "1,2345", "1234,567", "1,234.", "1,234.5,6", "1,,234", ",234", "1,", "١٢", "１２", "1.5E+2", "1.5E-2", "1.5E", "1.5e", "1E400", "1E-400", "-0", "-0.0", "+1", "\u{2212}42", "42\u{a0}", "1\u{a0}234", "1\u{202f}234", "1\u{2024}5", "1\u{ff0c}234", "1.5.", "..5", "1e3e4", "0.1e", "0000", "1,234e2", "1.5\u{200b}", "12 %", "12%", "Infinity", "-∞", "∞5", "NaN5", "nan", "1,234.567891234567890123", "123456789012345678901234567890"] as $s) p_4($f, $s);
echo "--de\n";
foreach (["1.234,5", "1,234.5", "1.234.567,89", "1.2345", "1 234", "1\u{a0}234"] as $s) p_4($d, $s);
echo "--nogroup\n";
foreach (["1,234", "1234", "1,234.5"] as $s) p_4($n, $s);
echo "--lenient\n";
foreach (["  42", "1,234", "1 234", "1,2,3", "(42)", "+42", "42abc", "1.234,5", "$42", "42$", "1,234.5", "1 234,5", "-  42", "1,2345", "1,234,56"] as $s) p_4($l, $s);
echo "--percent\n";
foreach (["12%", "12 %", "12", "12%%", "%12", "-12%", "1,200%", "12.5%", "0.5%"] as $s) p_4($pc, $s);
echo "--sci\n";
foreach (["1.5E3", "1.5E+3", "1.5", "1E3", "15E2", "1.5e3", "-1.5E3", "1.5E-3", "1,500E3"] as $s) p_4($sc, $s);
echo "--int only\n";
$io = new NumberFormatter("en_US", NumberFormatter::DECIMAL); $io->setAttribute(NumberFormatter::PARSE_INT_ONLY, 1);
foreach (["12.5", "12", "1,234.5", "1e3", "-12.5", ".5"] as $s) p_4($io, $s);
echo "--types\n";
foreach ([["1234.5", NumberFormatter::TYPE_INT32], ["1234.5", NumberFormatter::TYPE_INT64], ["2147483648", NumberFormatter::TYPE_INT32], ["-2147483649", NumberFormatter::TYPE_INT32], ["9223372036854775807", NumberFormatter::TYPE_INT64], ["9223372036854775807", NumberFormatter::TYPE_DOUBLE], ["-9223372036854775808", NumberFormatter::TYPE_INT64], ["-9223372036854775809", NumberFormatter::TYPE_INT64], ["1e19", NumberFormatter::TYPE_INT64], ["1e18", NumberFormatter::TYPE_INT64], ["∞", NumberFormatter::TYPE_INT64], ["NaN", NumberFormatter::TYPE_INT32], ["-0", NumberFormatter::TYPE_INT64], ["3.99999", NumberFormatter::TYPE_INT32], ["-3.9", NumberFormatter::TYPE_INT32], ["42", NumberFormatter::TYPE_DEFAULT], ["42.5", NumberFormatter::TYPE_DEFAULT], ["9223372036854775807", NumberFormatter::TYPE_DEFAULT], ["9223372036854775808", NumberFormatter::TYPE_DEFAULT], ["1e3", NumberFormatter::TYPE_DEFAULT], ["-0", NumberFormatter::TYPE_DEFAULT], ["12345678901234567890", NumberFormatter::TYPE_DEFAULT], ["0.1", NumberFormatter::TYPE_INT64], ["42", 99]] as [$s, $t]) { $p = 0; try { $r = $f->parse($s, $t, $p); echo json_encode($s), "/$t => ", var_export($r, true), " @$p err=", $f->getErrorCode(), "\n"; } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
echo "--pos\n";
foreach ([["1234", -1], ["1234", 4], ["1234", 5], ["1234", 2], ["12abc", 2], ["12abc", 3]] as [$s, $pos]) { $p = $pos; $r = $f->parse($s, NumberFormatter::TYPE_DOUBLE, $p); echo json_encode($s), " from $pos => ", var_export($r, true), " @$p err=", $f->getErrorCode(), "\n"; $p = $pos; $r = $f->parse($s, NumberFormatter::TYPE_INT64, $p); echo "   int64 => ", var_export($r, true), " @$p err=", $f->getErrorCode(), "\n"; }
echo "--parseCurrency\n";
foreach ([[$c, "$1,234.5"], [$c, "1,234.5"], [$c, "USD 1,234.5"], [$c, "USD1,234.5"], [$c, "€1,234.5"], [$c, "EUR 1,234.5"], [$c, "1,234.5 €"], [$c, "JP¥1,234"], [$c, "¥1,234"], [$c, "CA$1,234"], [$c, "-$1,234.5"], [$c, "($1,234.5)"], [$f, "$1,234.5"], [$f, "1,234.5"], [$d, "1.234,5 €"], [$d, "€ 1.234,5"], [$d, "1.234,5 $"], [$d, "1.234,5 US$"], [$d, "1.234,5 USD"], [$ac, "($1,234.5)"], [$ac, "-$1,234.5"], [$c, "US dollars 1,234.5"], [$c, "1,234.5 US dollars"]] as [$fm, $s]) { $cur = ""; $p = 0; $r = $fm->parseCurrency($s, $cur, $p); echo json_encode($s), " => ", var_export($r, true), " ", json_encode($cur), " @$p err=", $fm->getErrorCode(), "\n"; }


function p_5($f, $s, $lab = "") { $p = 0; $r = $f->parse($s, NumberFormatter::TYPE_DOUBLE, $p); echo $lab, json_encode($s), " => ", var_export($r, true), " @", $p, "\n"; }
$en = new NumberFormatter("en_US", NumberFormatter::DECIMAL);
$de = new NumberFormatter("de_DE", NumberFormatter::DECIMAL);
$fr = new NumberFormatter("fr_FR", NumberFormatter::DECIMAL);
$ch = new NumberFormatter("de_CH", NumberFormatter::DECIMAL);
$ar = new NumberFormatter("ar_EG", NumberFormatter::DECIMAL);
$hi = new NumberFormatter("hi_IN", NumberFormatter::DECIMAL);
$len = new NumberFormatter("en_US", NumberFormatter::DECIMAL); $len->setAttribute(NumberFormatter::LENIENT_PARSE, 1);
$lde = new NumberFormatter("de_DE", NumberFormatter::DECIMAL); $lde->setAttribute(NumberFormatter::LENIENT_PARSE, 1);
foreach (["1'234", "1\u{2019}234", "1\u{66b}234", "1\u{66c}234", "1\u{ff0e}234", "1\u{ff0c}234", "1\u{2024}234", "1\u{2024}5", "1\u{ff0e}5", "1\u{66b}5", "1\u{3002}5", "1\u{fe52}5", "1\u{fe50}234", "1\u{60c}234", "1\u{3001}234", "1,234\u{2024}5", "1\u{ff0c}234.5", "12,345", "123,456,789", "1,23", "1,234,5678", "01,234", "0,123", "1,234.56.7"] as $s) p_5($en, $s, "en ");
foreach (["1'234", "1\u{2019}234", "1.234", "1,5", "1.5", "1,234", "1\u{2024}234", "1\u{ff0c}5", "1\u{66b}5", "1.234.567", "1.234,567", "1.234.5"] as $s) p_5($de, $s, "de ");
foreach (["1\u{202f}234", "1\u{a0}234", "1 234", "1'234", "1,5", "1.5", "1\u{202f}234,5", "1\u{202f}234\u{202f}567", "1\u{2009}234", "1\u{2007}234"] as $s) p_5($fr, $s, "fr ");
foreach (["1\u{2019}234", "1'234", "1.234", "1,234", "1\u{2019}234.5", "1\u{2019}234,5"] as $s) p_5($ch, $s, "ch ");
foreach (["\u{661}\u{66c}\u{662}\u{663}\u{664}\u{66b}\u{665}", "1,234.5", "1\u{66c}234\u{66b}5", "1.234,5", "\u{661}\u{662}", "-\u{661}", "\u{61c}-\u{661}", "\u{661}\u{66c}\u{662}\u{663}\u{664}\u{66b}\u{665}%"] as $s) p_5($ar, $s, "ar ");
foreach (["12,34,567", "1,234,567", "12,345", "1,23,456", "123,456", "1234,567", "12,3456"] as $s) p_5($hi, $s, "hi ");
foreach (["1\u{a0}234", "1\u{2019}234", "1'234", "1.234,5", "1,234.5", "1.234.567", "1,234,567", "1 234 567", "1 234,5", "1.5", "1,5", "12 34", "1\u{66c}234", "1;234", "1_234"] as $s) p_5($len, $s, "len ");
foreach (["1.234,5", "1,234.5", "1 234,5", "1 234.5", "1.5", "1,5", "1,234,567", "1.234.567"] as $s) p_5($lde, $s, "lde ");
// currency parse trie
$c = new NumberFormatter("en_US", NumberFormatter::CURRENCY);
$dc = new NumberFormatter("de_DE", NumberFormatter::CURRENCY);
$lc = new NumberFormatter("en_US", NumberFormatter::CURRENCY); $lc->setAttribute(NumberFormatter::LENIENT_PARSE, 1);
foreach ([[$c, "EUR1,234.5"], [$c, "eur1,234.5"], [$c, "usd1,234.5"], [$c, "€1,234.5"], [$c, "US$1,234.5"], [$c, "A$1,234.5"], [$c, "AU$1,234.5"], [$c, "£1,234.5"], [$c, "CHF1,234.5"], [$c, "kr1,234.5"], [$c, "Kč1,234.5"], [$c, "₹1,234.5"], [$c, "1,234.5$"], [$c, "1,234.5USD"], [$c, "$1,234.5$"], [$c, "$$1,234.5"], [$c, "¤1,234.5"], [$c, "XXX1,234.5"], [$c, "ZZZ1,234.5"], [$c, "JPY1,234.5"], [$c, "¥1,234.5"], [$c, "CN¥1,234.5"], [$c, "元1,234.5"], [$dc, "1.234,5\u{a0}€"], [$dc, "1.234,5\u{a0}$"], [$dc, "1.234,5\u{a0}US$"], [$dc, "1.234,5\u{a0}USD"], [$dc, "1.234,5\u{a0}¥"], [$dc, "1.234,5\u{a0}CHF"], [$dc, "1.234,5\u{a0}Fr."], [$dc, "-1.234,5\u{a0}€"], [$dc, "€1.234,5"], [$lc, "$1,234.5"], [$lc, "1,234.5"], [$lc, "1,234.5 $"], [$lc, "US dollars 1,234.5"], [$lc, "1,234.5 US dollars"], [$lc, "1,234.5 euros"], [$lc, "USD 1,234.5"], [$lc, "$ 1,234.5"], [$lc, "1,234.5 dollars"], [$lc, "1,234.5 Euro"], [$lc, "€ 1,234.5"], [$lc, "-$1,234.5"], [$lc, "($1,234.5)"], [$lc, "$-1,234.5"], [$lc, "1,234.5€"]] as [$fm, $s]) { $cur = ""; $p = 0; $r = $fm->parseCurrency($s, $cur, $p); echo "pc ", json_encode($s), " => ", var_export($r, true), " ", json_encode($cur), " @$p\n"; }
foreach ([[$c, "EUR1,234.5"], [$c, "usd1,234.5"], [$c, "1,234.5USD"], [$c, "1,234.5$"], [$lc, "1,234.5"], [$lc, "1,234.5 $"], [$lc, "$ 1,234.5"], [$lc, "USD 1,234.5"], [$lc, "€1,234.5"], [$lc, "-$1,234.5"], [$lc, "$-1,234.5"], [$lc, "- $1,234.5"], [$lc, "1,234.5-"], [$lc, "1,234.5 US dollars"], [$lc, "US dollars 1,234.5"]] as [$fm, $s]) p_5($fm, $s, "cp ");


function p_6($f, $s, $lab = "") { $p = 0; $r = $f->parse($s, NumberFormatter::TYPE_DOUBLE, $p); echo $lab, json_encode($s), " => ", var_export($r, true), " @", $p, "\n"; }
$len = new NumberFormatter("en_US", NumberFormatter::DECIMAL); $len->setAttribute(NumberFormatter::LENIENT_PARSE, 1);
$lde = new NumberFormatter("de_DE", NumberFormatter::DECIMAL); $lde->setAttribute(NumberFormatter::LENIENT_PARSE, 1);
$lfr = new NumberFormatter("fr_FR", NumberFormatter::DECIMAL); $lfr->setAttribute(NumberFormatter::LENIENT_PARSE, 1);
$lhi = new NumberFormatter("hi_IN", NumberFormatter::DECIMAL); $lhi->setAttribute(NumberFormatter::LENIENT_PARSE, 1);
foreach (["1,5", "1,55", "1,555", "1,5555", "1,23,4", "1,234,5", "1,2,34", "12,3", "12,34", "1,234,56", "1,23,456", "1,234,567,8", "1,234,5678", "1 5", "1 55", "1 5555", "1'5", "1,", "1,234,", "1,,5", "1,234,,5", "1 ,234", "1, 234", ",5", " ,5", "1,5.5", "1,55.5", "1,555.5", "1.5,5", "1.5 5", "12.5.5"] as $s) p_6($len, $s, "len ");
foreach (["1.5", "1.55", "1.555", "1.5555", "1 5", "1 555", "1.555.5"] as $s) p_6($lde, $s, "lde ");
foreach (["1 5", "1 555", "1.5", "1.555", "1,5", "1'5", "1'555"] as $s) p_6($lfr, $s, "lfr ");
foreach (["12,34,567", "1,234,567", "1,23", "1,2345"] as $s) p_6($lhi, $s, "lhi ");
// strict with secondary grouping and other edge cases
$hi = new NumberFormatter("hi_IN", NumberFormatter::DECIMAL);
foreach (["1,234", "12,345", "123,456", "1,23,456", "12,34,56,789", "1,234,567", "1,23,4567"] as $s) p_6($hi, $s, "hi ");
$en = new NumberFormatter("en_US", NumberFormatter::DECIMAL);
foreach (["1,234,567.891", "١,٢٣٤", "1,٢٣٤", "1,234.٥", "0.5", "00.5", "-0.5", "-.5", "-", "-.", ".", ",", "-1,234", "1,234-", "5e", "5E+", "5E-", "5E1.5", "5E1,000", "5E01", "5E-01", "5E1e", "5.e1", ".5e1", "5e١", "5\u{ff25}1", "5\u{e9}1"] as $s) p_6($en, $s, "en ");
$g = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.###;(#)");
foreach (["(42", "(42)", "(42))", "((42))", "(1,234.5)", "(1,234.5)x"] as $s) p_6($g, $s, "paren ");
$s2 = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "'x'#,##0.###'y'");
foreach (["x42y", "x42", "42y", "42", "X42Y", "x42y1", "-x42y", "x-42y"] as $s) p_6($s2, $s, "xy ");
$s3 = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.###;'neg'#");
foreach (["neg42", "-42", "NEG42", "42"] as $s) p_6($s3, $s, "neg ");
$s4 = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "+#,##0.###;-#");
foreach (["+42", "-42", "42"] as $s) p_6($s4, $s, "plus ");
$s5 = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.###+;-#");
foreach (["42+", "-42", "42", "＋42", "42＋"] as $s) p_6($s5, $s, "plus2 ");
$s6 = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "*x#,##0.###");
foreach (["xxx42", "42", "x42x", "42xx", "-x42", "x-42"] as $s) p_6($s6, $s, "pad ");
$s7 = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.###;#-");
foreach (["42-", "-42", "42", "42−"] as $s) p_6($s7, $s, "trail ");
$s8 = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.###%;(#%)");
foreach (["42%", "(42%)", "42", "(42)", "42 %", "(42 %)"] as $s) p_6($s8, $s, "pct ");
$s9 = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "#,##0.###‰");
foreach (["42‰", "42", "42%"] as $s) p_6($s9, $s, "pml ");


$g = new NumberFormatter("en_US", NumberFormatter::DECIMAL); $g->setAttribute(NumberFormatter::ROUNDING_MODE, 7); var_dump($g->format(1234.5678), $g->getErrorCode(), $g->getErrorMessage(), $g->format(2), $g->getErrorCode());
$g->setAttribute(NumberFormatter::ROUNDING_MODE, 4);
foreach (["#,##0.###E0", "*x*y#", "#0#", "#,", "#.0#0", "#;#;#", "'abc", "#;", "#E", "#@", "@#@", "#,,#", ",#", "#.#.#", "#E0E0", "%#%", "#,##0.###¤¤¤¤¤¤", "#*", "", "#E+", "0.0E", "#,##0.###;", "a'b'c#", "#'", "*", "#*x", "**#", "@@,@@", "0,0", "#,#,#0", "#00.#", "0#", ".#", ".", "#.", "0.", "'#'#'#'", "#-#", "#¤#", "@@0", "@.#", "##0.0#@", "12", "#,##,##,##0"] as $pat) { $r = $g->setPattern($pat); echo json_encode($pat), " => ", var_export($r, true), " ", $r ? json_encode($g->getPattern()) . " " . $g->format(1234.5678) . " " . $g->format(0) . " " . $g->format(-0.5) : $g->getErrorMessage(), "\n"; $g->setPattern("#,##0.###"); }
$e = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL); var_dump($e->getPattern(), $e->format(1234.5), $e->format(0.5));
$e = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "bad{{"); var_dump($e->getPattern());
try { $e = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "'abc"); var_dump($e); } catch (Throwable $t) { echo get_class($t), ": ", $t->getMessage(), "\n"; } var_dump(intl_get_error_code(), intl_get_error_message());
var_dump(numfmt_create("en_US", NumberFormatter::PATTERN_DECIMAL, "'abc"), intl_get_error_code(), intl_get_error_message());
var_dump(numfmt_create("en_US", 99), intl_get_error_message());

$g = new NumberFormatter("en_US", NumberFormatter::DECIMAL); var_dump($g->setPattern("#;#;#"), $g->getErrorCode()); try { $x = new NumberFormatter("en_US", 99); var_dump($x); } catch (Throwable $t) { echo get_class($t), ": ", $t->getMessage(), "
"; } var_dump(intl_get_error_code()); try { $x = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, "#,,#"); } catch (Throwable $t) { echo get_class($t), ": ", $t->getMessage(), "
"; } var_dump(intl_get_error_code()); $h = new NumberFormatter("de_DE", NumberFormatter::DECIMAL); var_dump($h->setPattern("#¤#"), $h->getPattern(), $h->format(1234.5678), $h->format(-5), $h->getTextAttribute(NumberFormatter::CURRENCY_CODE), $h->getAttribute(NumberFormatter::MAX_FRACTION_DIGITS)); $c = new NumberFormatter("en_US", NumberFormatter::CURRENCY); var_dump($c->setTextAttribute(NumberFormatter::CURRENCY_CODE, "ZZ"), $c->getErrorMessage(), $c->setTextAttribute(NumberFormatter::CURRENCY_CODE, "EURO"), $c->getTextAttribute(NumberFormatter::CURRENCY_CODE), $c->setTextAttribute(NumberFormatter::CURRENCY_CODE, "eér"), $c->getErrorMessage(), $c->getTextAttribute(NumberFormatter::CURRENCY_CODE), $c->setTextAttribute(NumberFormatter::PADDING_CHARACTER, ""), $c->getTextAttribute(NumberFormatter::PADDING_CHARACTER), $c->setTextAttribute(NumberFormatter::PADDING_CHARACTER, "😀x"), $c->getTextAttribute(NumberFormatter::PADDING_CHARACTER), $c->setAttribute(NumberFormatter::FORMAT_WIDTH, 12), $c->format(5), $c->getPattern(), $c->getTextAttribute(NumberFormatter::DEFAULT_RULESET), $c->getErrorMessage(), $c->getTextAttribute(NumberFormatter::PUBLIC_RULESETS), $c->getErrorMessage(), $c->setTextAttribute(NumberFormatter::DEFAULT_RULESET, "x"), $c->getErrorMessage(), $c->getSymbol(-1), $c->getErrorMessage(), $c->getSymbol(18), $c->getErrorMessage(), $c->setSymbol(18, "x"), $c->getErrorMessage(), $c->setSymbol(NumberFormatter::CURRENCY_SYMBOL, ""), $c->format(5), $c->getSymbol(NumberFormatter::CURRENCY_SYMBOL), $c->setSymbol(NumberFormatter::ZERO_DIGIT_SYMBOL, "a"), $c->format(105), $c->setSymbol(NumberFormatter::ZERO_DIGIT_SYMBOL, "0"), $c->setSymbol(NumberFormatter::DECIMAL_SEPARATOR_SYMBOL, "<>"), $c->format(5.5), $c->setSymbol(NumberFormatter::MONETARY_SEPARATOR_SYMBOL, "#"), $c->format(5.5), $c->setSymbol(NumberFormatter::MINUS_SIGN_SYMBOL, "neg"), $c->format(-5.5), $c->setSymbol(NumberFormatter::EXPONENTIAL_SYMBOL, "x"), $c->setPattern("0.0E0"), $c->format(1234), $c->setSymbol(NumberFormatter::PERCENT_SYMBOL, "pct"), $c->setPattern("#%"), $c->format(0.5), $c->setSymbol(NumberFormatter::INFINITY_SYMBOL, "inf"), $c->format(INF), $c->setSymbol(NumberFormatter::NAN_SYMBOL, "nan"), $c->format(NAN), $c->parse("inf"), $c->parse("nan"), $c->parse("neg5"));

$g = new NumberFormatter("en_US", NumberFormatter::DECIMAL);
foreach (["#,##0¤", "#,##0¤¤", "#¤", "#¤#", "#¤00", "#,##0.00¤", "#¤ x", "#¤;-#", "0¤0", "#¤.#"] as $pat) { $r = $g->setPattern($pat); echo json_encode($pat), " => ", var_export($r, true), " ", $r ? json_encode($g->getPattern()) . " " . $g->format(1234.5678) . " " . $g->format(5) . " " . $g->format(-5.5) . " cc=" . $g->getTextAttribute(NumberFormatter::CURRENCY_CODE) . " frac=" . $g->getAttribute(NumberFormatter::MIN_FRACTION_DIGITS) . "/" . $g->getAttribute(NumberFormatter::MAX_FRACTION_DIGITS) . " das=" . $g->getAttribute(NumberFormatter::DECIMAL_ALWAYS_SHOWN) . " ps=" . json_encode($g->getTextAttribute(NumberFormatter::POSITIVE_SUFFIX)) : $g->getErrorMessage(), "\n"; $g->setPattern("#,##0.###"); }

$g = new NumberFormatter("en_US", NumberFormatter::PATTERN_DECIMAL, ""); var_dump($g->format(1/3), $g->format(1e-7), $g->format(123456789.123456789), $g->getAttribute(NumberFormatter::MAX_FRACTION_DIGITS), $g->getAttribute(NumberFormatter::MIN_FRACTION_DIGITS), $g->getAttribute(NumberFormatter::MAX_INTEGER_DIGITS), $g->getAttribute(NumberFormatter::GROUPING_USED), $g->getAttribute(NumberFormatter::GROUPING_SIZE), $g->getAttribute(NumberFormatter::ROUNDING_MODE), $g->getAttribute(NumberFormatter::MULTIPLIER)); $g->setAttribute(NumberFormatter::MULTIPLIER, 3); $g->setAttribute(NumberFormatter::ROUNDING_MODE, 2); $g->setAttribute(NumberFormatter::LENIENT_PARSE, 1); $g->setTextAttribute(NumberFormatter::CURRENCY_CODE, "EUR"); $g->setPattern(""); var_dump($g->getAttribute(NumberFormatter::MULTIPLIER), $g->getAttribute(NumberFormatter::ROUNDING_MODE), $g->getAttribute(NumberFormatter::LENIENT_PARSE), $g->getTextAttribute(NumberFormatter::CURRENCY_CODE), $g->getSymbol(NumberFormatter::CURRENCY_SYMBOL)); $c = new NumberFormatter("en_US", NumberFormatter::CURRENCY); $c->setPattern(""); var_dump($c->getPattern(), $c->format(5), $c->getTextAttribute(NumberFormatter::CURRENCY_CODE), $c->getAttribute(NumberFormatter::MAX_FRACTION_DIGITS)); $s = new NumberFormatter("en_US", NumberFormatter::SCIENTIFIC); var_dump($s->format(0), $s->format(-0.0), $s->format(NAN), $s->format(INF), $s->format(-INF), $s->format(123456), $s->format(0.000123), $s->format(1e300), $s->format(-1e-300), $s->format(999999), $s->format(9.999)); $s->setPattern("00.##E0"); var_dump($s->format(0), $s->format(5), $s->format(123456), $s->format(0.0123)); $s->setPattern("#,##0.00E0"); var_dump($s->getErrorCode()); $s->setPattern("0.0##E0"); $s->setAttribute(NumberFormatter::MAX_SIGNIFICANT_DIGITS, 3); var_dump($s->getPattern(), $s->format(123456)); $s->setPattern("0.000E0"); var_dump($s->format(999999), $s->format(0.1), $s->format(1)); $s->setPattern("##0.000E0"); var_dump($s->format(999999), $s->format(99999), $s->format(0.1), $s->format(1), $s->format(-12345.678)); $s->setPattern("###0.0E0"); var_dump($s->format(999999), $s->format(12345.678), $s->format(0.00012)); $s->setPattern("0.###E0"); var_dump($s->format(0.5), $s->format(10), $s->format(9.9999), $s->format(1234.5)); $s->setPattern("#.###E0"); var_dump($s->format(1234.5), $s->format(0), $s->format(12345.678)); $s->setPattern("00.000E0"); var_dump($s->format(1234.5), $s->format(0), $s->format(5));
