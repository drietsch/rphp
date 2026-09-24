<?php
// mb_convert_kana: every mode letter over the BMP (hashed), the voiced-
// mark glue, the flag errors and their order, SJIS / EUC-JP /
// ISO-2022-JP input, substitute characters.
$all = '';
for ($c = 0; $c < 0x10000; $c++) {
    if ($c >= 0xD800 && $c <= 0xDFFF) {
        continue;
    }
    $all .= mb_chr($c);
}
$pairs = '';
for ($c = 0xFF61; $c <= 0xFF9F; $c++) {
    $pairs .= mb_chr($c) . "\u{FF9E}" . mb_chr($c) . "\u{FF9F}" . mb_chr($c) . 'x';
}
foreach (['A', 'a', 'R', 'r', 'N', 'n', 'S', 's', 'K', 'k', 'H', 'h', 'C', 'c', 'M', 'm', 'V', 'KV', 'HV', 'KVA', 'hk',
          'Kc', 'HC', 'KVCS', 'ashk', 'KVSN', 'RNSKHV', ''] as $m) {
    echo str_pad($m, 7), md5(mb_convert_kana($all, $m)), " ", md5(mb_convert_kana($pairs, $m)), "\n";
}
echo md5(mb_convert_kana($all)), "\n";
var_dump(mb_convert_kana("ｶﾞｷﾞﾊﾟｳﾞ ｱ"), mb_convert_kana("ｶﾞｷﾞﾊﾟｳﾞ ｱ", "K"), mb_convert_kana("ｶﾞｷﾞﾊﾟｳﾞ ｱ", "HV"));
var_dump(mb_convert_kana("ガヴパ", "k"), mb_convert_kana("がぱ", "h"), mb_convert_kana("ＡＢＣ　１２３", "as"));

echo "-- encodings\n";
foreach (['SJIS', 'EUC-JP', 'SJIS-win', 'eucJP-win', 'CP932', 'ISO-2022-JP'] as $e) {
    $s = mb_convert_encoding("ｶﾞｷﾞﾊﾟ アイウ　ＡＢＣ１２３ abc 123 ", $e, 'UTF-8');
    foreach (['KV', 'k', 'h', 'a', 'A', 'rns', 'RNS', 'c', 'C', 'HV'] as $m) {
        echo $e, " $m ", bin2hex(mb_convert_kana($s, $m, $e)), "\n";
    }
}
var_dump(mb_convert_kana("a\xffb", 'A'));
mb_substitute_character('none');
var_dump(mb_convert_kana("a\xffb", 'A'));
mb_substitute_character(0x3013);
var_dump(mb_convert_kana("a\xffb", 'A'));

echo "-- errors\n";
foreach (['q', 'Aa', 'Rr', 'Nn', 'Ss', 'Kk', 'Hh', 'Cc', 'Mm', 'Ww', 'HK', 'aR', 'Ar', 'An', 'Na', 'ck', 'Ch', 'kC', 'hc',
          'AaRr', 'RrNn', 'CcMm', 'aRnN', 'Nna', 'KHkc', 'kchC', 'HKCc', 'kCh', 'ckh', 'hCHK', 'KHMm', "K\0", 'v'] as $mode) {
    try {
        mb_convert_kana("abc", $mode);
        echo json_encode($mode), " ok\n";
    } catch (\ValueError $e) {
        echo json_encode($mode), " ", $e->getMessage(), "\n";
    }
}
try {
    mb_convert_kana("abc", "K", "foo");
} catch (\ValueError $e) {
    echo $e->getMessage(), "\n";
}
