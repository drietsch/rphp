<?php
// IntlListFormatter: types × widths across locales, ICU's Spanish and
// Hebrew contextual conjunctions, the argument checks.
$types = [IntlListFormatter::TYPE_AND, IntlListFormatter::TYPE_OR, IntlListFormatter::TYPE_UNITS];
$widths = [IntlListFormatter::WIDTH_WIDE, IntlListFormatter::WIDTH_SHORT, IntlListFormatter::WIDTH_NARROW];
foreach (['en', 'en_GB', 'de', 'fr', 'es', 'ja', 'zh', 'ar', 'he', 'mi', 'ti', 'ru', 'hi'] as $l) {
    foreach ($types as $t) {
        foreach ($widths as $w) {
            $f = new IntlListFormatter($l, $t, $w);
            echo "$l $t $w: ", $f->format(['A']), ' | ', $f->format(['A', 'B']), ' | ', $f->format(['A', 'B', 'C']), ' | ', $f->format(['A', 'B', 'C', 'D']), "\n";
        }
    }
}
foreach (['ibis', 'hielo', 'Hierro', 'ocho', '8', '11', '11 x', '110', 'hola', 'o', 'Iris'] as $w) {
    echo (new IntlListFormatter('es'))->format(['x', $w]), ' / ', (new IntlListFormatter('es', IntlListFormatter::TYPE_OR))->format(['a', 'b', $w]), "\n";
}
echo (new IntlListFormatter('he'))->format(['א', 'abc']), ' / ', (new IntlListFormatter('he'))->format(['א', 'ב']), "\n";
$f = new IntlListFormatter('en');
var_dump($f->format([]), $f->format([1, 2.5, true, null]), $f->format(['k' => 'a', 5 => 'b']));
var_dump($f->format(['a', "\xff"]), $f->getErrorCode(), $f->getErrorMessage(), intl_get_error_code());
var_dump($f->format(['a', 'b']), $f->getErrorCode());
foreach (['xx', 'root', 'i-klingon', '123'] as $l) {
    try {
        new IntlListFormatter($l);
    } catch (ValueError $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
try {
    new IntlListFormatter('en', 5);
} catch (ValueError $e) {
    echo $e->getMessage(), "\n";
}
try {
    new IntlListFormatter('en', 0, 7);
} catch (ValueError $e) {
    echo $e->getMessage(), "\n";
}
try {
    clone $f;
} catch (Error $e) {
    echo $e->getMessage(), "\n";
}
