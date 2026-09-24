<?php
// Spoofchecker: restriction levels, mixed numbers, invisible marks, the
// hidden-overlay dot, allowed locales and characters, confusables.
$strs = ["paypal", "pаypal", "Рaypal", "раураl", "hello", "hello world", "123", "١٢٣", "1٢3", "𝟏23", "abc١", "日本語", "日本語abc", "ひらがなカタカナ漢字abc", "한국어abc", "中文ㄅㄆabc", "Ελληνικά", "Ελληνικάabc", "Привет", "Приветabc", "שלום", "abcשלום", "i̇", "ı̇", "ȷ̇", "l̇", "ì̇", "é", "é", "á́", "á̀", "a\u{0301}\u{0301}", "a\u{0301}\u{0300}\u{0301}", "\u{200B}", "a\u{200D}b", "ᏚᎢᎵᎬᎢᎬᏒ", "abcᏚ", "", " ", "x-y_z", "O0", "l1I|", "rn", "m", "ǉ", "ﬁ", "Ⅰ", "ℌ", "ａｂｃ", "🙂", "a🙂", "Iñtërnâtiônàlizætiøn", "ⅰⅱ", "٣3", "۳3", "٣۳"];
$pairs = [["paypal", "pаypal"], ["paypal", "paypal"], ["scope", "ѕсоре"], ["ѕсоре", "ѕсоре"], ["rn", "m"], ["l", "1"], ["l", "I"], ["O", "0"], ["abc", "def"], ["Ρ", "P"], ["ΡΑ", "PA"], ["Хреп", "Xpen"], ["ǉ", "lj"], ["ﬁ", "fi"], ["", ""], ["a", ""], ["١", "l"], ["日", "曰"], ["cafe\u{0301}", "café"], ["𝟏", "1"], ["x", "×"], ["-", "‐"], ["ｘ", "x"]];
$s = new Spoofchecker();
foreach ($strs as $t) { $e = null; $r = $s->isSuspicious($t, $e); echo json_encode($t, JSON_UNESCAPED_UNICODE), " => ", var_export($r, true), " ", $e, "\n"; }
foreach ($pairs as [$a, $b]) { $e = null; $r = $s->areConfusable($a, $b, $e); echo "$a|$b => ", var_export($r, true), " ", $e, "\n"; }
foreach ([Spoofchecker::SINGLE_SCRIPT_CONFUSABLE, Spoofchecker::MIXED_SCRIPT_CONFUSABLE, Spoofchecker::WHOLE_SCRIPT_CONFUSABLE, Spoofchecker::INVISIBLE, Spoofchecker::MIXED_NUMBERS, Spoofchecker::SINGLE_SCRIPT, Spoofchecker::CHAR_LIMIT, Spoofchecker::HIDDEN_OVERLAY, Spoofchecker::ANY_CASE, 0, 0xFFFF] as $c) {
    $s = new Spoofchecker(); $s->setChecks($c);
    $out = [];
    foreach ($strs as $t) { $e = null; $r = $s->isSuspicious($t, $e); $out[] = (int)$r . ':' . $e; }
    foreach ($pairs as [$a, $b]) { $e = null; $r = @$s->areConfusable($a, $b, $e); $out[] = var_export($r, true) . ':' . var_export($e, true); }
    echo "checks $c: ", implode(' ', $out), "\n";
}
foreach ([Spoofchecker::ASCII, Spoofchecker::SINGLE_SCRIPT_RESTRICTIVE, Spoofchecker::HIGHLY_RESTRICTIVE, Spoofchecker::MODERATELY_RESTRICTIVE, Spoofchecker::MINIMALLY_RESTRICTIVE, Spoofchecker::UNRESTRICTIVE] as $lv) {
    $s = new Spoofchecker(); $s->setRestrictionLevel($lv);
    $out = [];
    foreach ($strs as $t) { $e = null; $r = $s->isSuspicious($t, $e); $out[] = (int)$r . ':' . $e; }
    echo "level $lv: ", implode(' ', $out), "\n";
}
foreach (['en', 'en_US, ja', 'ru', 'de_DE', 'el, zh', ''] as $loc) {
    $s = new Spoofchecker(); @$s->setAllowedLocales($loc);
    $out = [];
    foreach ($strs as $t) { $e = null; $r = $s->isSuspicious($t, $e); $out[] = (int)$r . ':' . $e; }
    echo "locales $loc: ", implode(' ', $out), "\n";
}
$s = new Spoofchecker(); $s->setAllowedChars('[a-z0-9]');
foreach (["abc", "ABC", "a-b", "ab9"] as $t) { $e = null; var_dump($s->isSuspicious($t, $e), $e); }
try { $s->setAllowedChars('abc'); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { $s->setAllowedChars('[a-z]', 99); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { $s->setRestrictionLevel(5); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
