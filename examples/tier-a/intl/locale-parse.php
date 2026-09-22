<?php
// Tier-A differential: Locale's subtag accessors follow ICU's uloc_* parsing
// (separators `_`/`-`, the `@keyword=value` tail, case normalisation, the
// 3-letter → 2-letter language and region tables), and parseLocale /
// composeLocale / getAllVariants / getKeywords / canonicalize on top.
$inputs = ['en', 'en_US', 'en-us', 'EN_us', 'de_DE@currency=EUR;collation=PHONEBOOK', 'sr-Latn-RS', 'sr_latn_rs',
    'zh-Hans-CN', 'zh_hant_TW', 'en_US_POSIX', 'de-DE-1996', 'sl_IT_nedis_rozaj', 'es-419', 'es_419_valencia',
    'eng-USA', 'deu', 'iw_IL', 'in', 'ji', 'jw', 'no_NO_NY', 'ca-ES-valencia', 'ar-001', 'root', '', 'x-private',
    'de-DE-u-co-phonebk', 'en-a-bbb-x-a-ccc', 'i-klingon', 'zh-min-nan', 'art-lojban', 'sgn-BE-FR',
    'und', 'und_Latn', 'en_US.UTF-8', 'en_US@calendar=gregorian', 'de__PHONEBOOK', 'fr-Latn', 'a', 'abcd', 'abcdefghi_XX',
    'en-US-x-twain', 'de-Latn-DE-1996-x-abc', 'zh-cmn-Hans-CN', 'tlh', 'nb-NO', 'nn_NO', 'mo', 'sh', 'sh_YU', 'th_TH_TH', 'ja_JP_TRADITIONAL',
    'hy_AM_REVISED', 'en_Latn_US_POSIX', 'FR-fr', 'de-AT-u-va-posix', 'pt_PT_'];
foreach ($inputs as $in) {
    echo "[$in] lang=", var_export(Locale::getPrimaryLanguage($in), true),
        " script=", var_export(Locale::getScript($in), true),
        " region=", var_export(Locale::getRegion($in), true),
        " variants=", json_encode(Locale::getAllVariants($in)),
        " keywords=", json_encode(Locale::getKeywords($in)),
        " canonical=", var_export(Locale::canonicalize($in), true), "\n";
    echo "  parse=", json_encode(Locale::parseLocale($in)), "\n";
}
var_dump(Locale::composeLocale(['language' => 'en', 'script' => 'Latn', 'region' => 'US', 'variant0' => 'POSIX']));
var_dump(Locale::composeLocale(['language' => 'de', 'region' => 'DE', 'variant' => ['1996', 'x'], 'private0' => 'abc', 'private1' => 'def']));
var_dump(Locale::composeLocale(['language' => 'sl', 'extlang0' => 'abc', 'variant' => 'nedis', 'private' => 'p1']));
var_dump(Locale::composeLocale(['grandfathered' => 'i-klingon', 'language' => 'x']));
try { var_dump(Locale::composeLocale(['region' => 'US'])); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
