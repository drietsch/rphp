<?php
// Dumps what ICU's DateTimePatternGenerator is built from, for every
// locale the stock php/ICU this project is measured against knows:
//   - the calendar's `availableFormats` (skeleton → pattern) in the order
//     ICU's AvailableFormatsSink sees them: the locale's bundle first, then
//     its `%%Parent`/truncated parents, root last (flagged); a calendar
//     other than the Gregorian one reads its own table, then root's alias
//     target, the `generic` calendar (a plural-variant table is the empty
//     pattern ICU gets from it);
//   - the calendar's `DateTimePatterns` (the four time and four date style
//     patterns);
//   - the `appendItems` formats and the `fields/*/dn` display names, each
//     key from the first bundle in the chain that has it;
//   - the date-time glue for the full, long, medium and short styles, the
//     hour character of `j`, the hour and day-period characters of `C`,
//     and the decimal separator, read back from the generator itself.
// Rows are deduplicated; a locale points at its row.
//
//   php -n tools/intl-data/dump-patgen.php > crates/rphp-ext-intl/src/patgen/data.rs
$all = ResourceBundle::getLocales('');
$all[] = 'en_US_POSIX';
sort($all);
$bundles = [];
$open = function (string $b) use (&$bundles) {
    if (!array_key_exists($b, $bundles)) $bundles[$b] = @new ResourceBundle($b, NULL, false) ?: null;
    return $bundles[$b];
};
$chain = function (string $loc) use ($open) {
    $out = [];
    for ($b = $loc; ; ) {
        $rb = $open($b);
        if ($rb) $out[] = $b;
        if ($b === 'root') break;
        $parent = $rb ? $rb->get('%%Parent', false) : null;
        if (is_string($parent) && $parent !== '') { $b = $parent; continue; }
        $i = strrpos($b, '_');
        $b = $i === false ? 'root' : substr($b, 0, $i);
    }
    return $out;
};
// a resource path in one bundle, no fallback
$at = function (string $b, array $path) use ($open) {
    $r = $open($b);
    foreach ($path as $k) {
        if (!$r instanceof ResourceBundle) return null;
        $r = $r->get($k, false);
        if ($r === null || $r === false) return null;
    }
    return $r;
};
$rows = []; $index = []; $locales = [];
$appendKeys = ['Era', 'Year', 'Quarter', 'Month', 'Week', '*', 'Day-Of-Week', '*', '*', 'Day', '*', 'Hour', 'Minute', 'Second', '*', 'Timezone'];
$fieldKeys = ['era', 'year', 'quarter', 'month', 'week', 'weekOfMonth', 'weekday', 'dayOfYear', 'weekdayOfMonth', 'day', 'dayperiod', 'hour', 'minute', 'second', '*', 'zone'];
foreach (array_unique($all) as $loc) {
    $cal = IntlCalendar::createInstance(null, $loc)->getType();
    $ch = $chain($loc);
    // a path under the calendar: the calendar's own tables below root, then
    // (root's alias) the generic calendar's, root included
    $types = $cal === 'gregorian' ? ['gregorian'] : [$cal, 'generic'];
    $walk = function (array $rest) use ($types, $ch, $at) {
        $out = [];
        foreach ($types as $ti => $type) foreach ($ch as $b) {
            if ($b === 'root' && $ti === 0 && count($types) > 1) continue;
            $out[] = [$b, $at($b, array_merge(['calendar', $type], $rest))];
        }
        return $out;
    };
    // availableFormats in sink order
    $avail = []; $seen = [];
    foreach ($walk(['availableFormats']) as [$b, $t]) {
        if (!$t instanceof ResourceBundle) continue;
        $items = [];
        foreach ($t as $k => $v) $items[$k] = is_string($v) ? $v : '';
        ksort($items, SORT_STRING);
        foreach ($items as $k => $v) {
            if (isset($seen[$k])) continue;
            $seen[$k] = true;
            $avail[] = [$k, $v, $b === 'root'];
        }
    }
    // style patterns: the first bundle with them
    $dtp = null;
    foreach ($walk(['DateTimePatterns']) as [$b, $t]) {
        if ($t instanceof ResourceBundle) { $dtp = []; foreach ($t as $p) $dtp[] = is_string($p) ? $p : $p->get(0); break; }
    }
    $styles = array_slice($dtp, 0, 8);
    // appendItems and field names, per key
    $append = [];
    foreach ($appendKeys as $k) {
        $v = '';
        if ($k !== '*') foreach ($walk(['appendItems', $k]) as [$b, $x]) {
            if (is_string($x)) { $v = $x; break; }
        }
        $append[] = $v;
    }
    $names = [];
    foreach ($fieldKeys as $k) {
        $v = '';
        if ($k !== '*') foreach ($ch as $b) {
            $x = $at($b, ['fields', $k, 'dn']);
            if (is_string($x)) { $v = $x; break; }
        }
        $names[] = $v;
    }
    // read back from the generator: the glue, the hour characters
    $g = new IntlDatePatternGenerator($loc);
    $time = $g->getBestPattern('Hm');
    $glue = [];
    foreach (['yMMMMEEEEd', 'yMMMMd', 'yMMMd', 'yMd'] as $ds) {
        $date = $g->getBestPattern($ds);
        $both = $g->getBestPattern($ds . 'Hm');
        $x = str_replace([$date, $time], ["\u{E001}", "\u{E000}"], $both);
        if (substr_count($x, "\u{E001}") !== 1 || substr_count($x, "\u{E000}") !== 1) throw new Exception("$loc: cannot read the glue of $ds: $both");
        $glue[] = str_replace(["\u{E001}", "\u{E000}"], ['{1}', '{0}'], $x);
    }
    $j = $g->getBestPattern('j');
    preg_match('/[hHkK]/', $j, $m); $jh = $m[0];
    $c = $g->getBestPattern('C');
    preg_match('/[hHkK]/', $c, $m); $ch_ = $m[0];
    $cp = preg_match('/[abB]/', preg_replace("/'[^']*'/", '', $c), $m) ? $m[0] : 'a';
    $dec = (new NumberFormatter($loc, NumberFormatter::DECIMAL))->getSymbol(NumberFormatter::DECIMAL_SEPARATOR_SYMBOL);
    $row = [$avail, $styles, $append, $names, $glue, $jh . $ch_ . $cp, $dec];
    $key = json_encode($row);
    if (!isset($index[$key])) { $index[$key] = count($rows); $rows[] = $row; }
    $locales[$loc] = $index[$key];
}
$q = fn($s) => '"' . preg_replace_callback('/[\p{Cf}\x{00A0}\x{2000}-\x{200F}\x{2028}-\x{202F}\x{205F}-\x{206F}\x{3000}\x{FEFF}]/u', fn($m) => sprintf('\\u{%X}', mb_ord($m[0], 'UTF-8')), addcslashes($s, "\\\"\0..\37")) . '"';
$list = fn($a) => '[' . implode(', ', array_map($q, $a)) . ']';
echo "//! Generated by `tools/intl-data/dump-patgen.php` from php ", PHP_VERSION, " / ICU ", INTL_ICU_VERSION, " — do not edit.\n";
echo "//! What ICU's DateTimePatternGenerator is built from, per locale (see the script).\n\n";
echo "pub struct Row {\n    /// (skeleton, pattern, from root) in the order ICU adds them.\n    pub avail: &'static [(&'static str, &'static str, bool)],\n";
echo "    /// The time styles full…short, then the date styles full…short.\n    pub styles: [&'static str; 8],\n";
echo "    /// `appendItems` by field (empty: none).\n    pub append: [&'static str; 16],\n    /// Field display names (empty: none).\n    pub names: [&'static str; 16],\n";
echo "    /// The date-time glue for full, long, medium, short.\n    pub glue: [&'static str; 4],\n";
echo "    /// The hour character of `j`, then the hour and day-period characters of `C`.\n    pub hours: &'static str,\n    pub decimal: &'static str,\n}\n\n";
echo "pub static ROWS: &[Row] = &[\n";
foreach ($rows as [$avail, $styles, $append, $names, $glue, $hours, $dec]) {
    echo "    Row {\n        avail: &[", implode(', ', array_map(fn($a) => '(' . $q($a[0]) . ', ' . $q($a[1]) . ', ' . ($a[2] ? 'true' : 'false') . ')', $avail)), "],\n";
    echo "        styles: ", $list($styles), ",\n        append: ", $list($append), ",\n        names: ", $list($names), ",\n        glue: ", $list($glue), ",\n";
    echo "        hours: ", $q($hours), ",\n        decimal: ", $q($dec), ",\n    },\n";
}
echo "];\n\n/// Locale → row, sorted by locale for a binary search.\npub static LOCALES: &[(&str, u16)] = &[\n";
foreach ($locales as $loc => $i) { echo "    (", $q($loc), ", $i),\n"; }
echo "];\n";
