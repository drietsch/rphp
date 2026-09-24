<?php
// mb_regex_encoding: the names Oniguruma accepts and the names it reports;
// matching, replacing, splitting and searching in SJIS / EUC-JP /
// ISO-8859-x / UTF-16 / UCS-4 subjects with byte offsets in those
// encodings; CJK encodings' ASCII-only ctypes.
var_dump(mb_regex_encoding());
foreach (['utf8', 'Shift_JIS', 'sjis-win', 'CP932', 'eucjp-win', 'EUC-JP', 'UTF-16', 'UTF-16LE', 'UCS-4', 'UTF-32LE',
          'BIG5', 'CP950', 'EUC-CN', 'GB2312', 'EUC-TW', 'EUC-KR', 'UHC', 'KOI8-R', 'KOI8-U', 'ISO-8859-1', 'latin1',
          'ISO-8859-15', 'ISO-8859-12', 'ASCII', 'us-ascii', 'Windows-1252', 'pass', 'auto', ''] as $e) {
    try {
        $r = mb_regex_encoding($e);
        echo "$e => ", var_export($r, true), " now ", mb_regex_encoding(), "\n";
    } catch (\ValueError $ex) {
        echo "$e => ", $ex->getMessage(), "\n";
    }
}
mb_regex_encoding('UTF-8');
mb_internal_encoding('EUC-JP');
var_dump(mb_regex_encoding());
mb_internal_encoding('UTF-8');

function h($x) { return is_string($x) ? bin2hex($x) : var_export($x, true); }
function show($r) {
    if (!is_array($r)) {
        return h($r);
    }
    $o = [];
    foreach ($r as $k => $v) {
        $o[] = "$k=" . h($v);
    }
    return '[' . implode(',', $o) . ']';
}
foreach (['SJIS', 'EUC-JP', 'ISO-8859-1', 'ISO-8859-7', 'UTF-16LE', 'UTF-16', 'UCS-4', 'EUC-KR', 'EUC-CN', 'KOI8-R', 'ASCII'] as $enc) {
    mb_regex_encoding($enc);
    echo "== $enc ", mb_regex_encoding(), "\n";
    $cv = fn($s) => mb_convert_encoding($s, $enc, 'UTF-8');
    $subj = $cv("abcあいうéΣ日本語 xyzé");
    foreach (['い(う)', '[[:alpha:]]+', '\w+', '(?<n>本)語', 'é+', '.', '[^a-z]+', 'Σ', '\xe9', 'x{2}'] as $p) {
        $r = @mb_ereg($cv($p), $subj, $m);
        echo "  ", $p, " => ", h($r), " ", show($m), "\n";
    }
    echo "  repl ", h(@mb_ereg_replace($cv('(い)'), $cv('[\1\k<1>]'), $subj)), "\n";
    echo "  split ", show(mb_split($cv('う'), $subj)), "\n";
    mb_ereg_search_init($subj, $cv('[あ-う]'));
    while ($p = mb_ereg_search_pos()) {
        echo "  pos ", show($p), "\n";
    }
    echo "  bad ", h(mb_ereg('a', "\xff\xfe\xfd")), "\n";
}
mb_regex_encoding('EUC-JP');
$s = mb_convert_encoding('(?<名>日)本', 'EUC-JP', 'UTF-8');
var_dump(mb_ereg($s, mb_convert_encoding('日本', 'EUC-JP', 'UTF-8'), $m), array_map('bin2hex', array_keys($m)));
