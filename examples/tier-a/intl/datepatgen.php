<?php
// IntlDatePatternGenerator: getBestPattern() over skeletons and locales —
// the hour-cycle metacharacters, field-length adjustment, appended fields,
// the date-time glue.
$skeletons = ['yMMMd', 'jmm', 'Jmm', 'Cmm', 'hmm', 'Hmm', 'yMMMMEEEEd', 'yMd', 'E', 'EEEE', 'ddMMyy', 'yyyyMMdd',
    'mmss', 'ms', 'mmssSSS', 'yMMMdjmm', 'yMMMMdjmm', 'yMMMMEEEEdjmm', 'yMdjmm', 'GyMMMd', 'yQQQ', 'MMM', 'Ehm',
    'vvvvjmm', 'Bhm', 'yD', 'yw', 'yyMMMdd', 'MMMMM', 'uuuu', 'jjjjmm', 'hhmmss', 'sS', 'MEd', 'xyz', '', 'yMMMd HH:mm'];
foreach (['en', 'en_GB', 'de', 'fr', 'ja', 'zh_Hant', 'ko', 'ru', 'ar', 'he', 'hi', 'th', 'fa', 'es_MX', 'pt_BR', 'sv'] as $l) {
    $g = new IntlDatePatternGenerator($l);
    $out = [];
    foreach ($skeletons as $s) {
        $out[] = $g->getBestPattern($s);
    }
    echo $l, ': ', implode(' | ', $out), "\n";
}
$g = IntlDatePatternGenerator::create('de_CH');
var_dump(get_class($g), $g->getBestPattern('yMMMdHm'));
$c = clone $g;
var_dump($c->getBestPattern('jmm'));
var_dump((new IntlDatePatternGenerator())->getBestPattern('yMd'));
try {
    $g->__construct('en');
} catch (IntlException $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
var_dump($g->getBestPattern("\xff"), intl_get_error_code(), intl_get_error_message());
