<?php
// Transliterator: the IDs Symfony's slugger and ascii() use, script
// transliterators, compound IDs with filters, the ID php reports.
$text = "Grüße aus Köln — Ærøskøbing ½ “quoted” Привет мир Ελληνικά 北京市 مرحبا שלום ひらがな カタカナ 한국어";
foreach ([
    'Any-Latin; Latin-ASCII',
    'Any-Latin; Latin-ASCII; Lower()',
    'Latin-ASCII',
    'de-ASCII',
    'NFD; [:Nonspacing Mark:] Remove; NFC',
    'Any-Lower', 'Any-Upper', 'Any-Title',
    'Russian-Latin/BGN', 'Ukrainian-Latin/BGN', 'Greek-Latin', 'Arabic-Latin',
    'Han-Latin', 'Hiragana-Latin', 'Katakana-Hiragana', 'Hangul-Latin',
    'Cyrillic-Latin', 'Latin-Cyrillic', 'Any-Latin', 'Fullwidth-Halfwidth',
    '[:^ASCII:] Any-Hex', 'Any-Hex/XML', 'Any-Name',
    ' any-latin ; latin-ascii ',
] as $id) {
    $t = Transliterator::create($id);
    echo $id, ' => [', $t->id, "]\n  ", $t->transliterate($text), "\n";
}
var_dump(transliterator_transliterate('Any-Latin; Latin-ASCII', 'Ďábelské ódy'));
var_dump(Transliterator::create('Hex-Any')->transliterate('é A'));
var_dump(Transliterator::create('Name-Any')->transliterate('\N{LATIN SMALL LETTER E WITH ACUTE}!'));
// the slugger's locale IDs
foreach (['Amharic-Latin', 'Belarusian-Latin/BGN', 'Bulgarian-Latin/BGN', 'Georgian-Latin/BGN', 'Armenian-Latin/BGN', 'Macedonian-Latin/BGN', 'Serbian-Latin/BGN', 'Kazakh-Latin/BGN', 'Persian-Latin/BGN', 'Hebrew-Latin/BGN', 'Thai-Latin'] as $id) {
    $t = Transliterator::create($id);
    echo $id, ': ', $t ? $t->transliterate('Мир ქართული Հայերեն سلام שלום ሰላም ประเทศไทย') : 'null', "\n";
}
// ranges are UTF-16 code units
$t = Transliterator::create('Any-Upper');
var_dump($t->transliterate('abcdef', 2, 4), $t->transliterate('aé😀b', 1, 4), $t->transliterate('abcdef', 3));
// inverses and reverse creation
foreach (['Latin-Cyrillic', 'Any-Latin; Latin-ASCII', 'Lower', 'NFD', '[a-z] Remove', 'Any-Latin; Title', 'Hiragana-Katakana'] as $id) {
    $t = Transliterator::create($id);
    $inv = $t->createInverse();
    $rev = Transliterator::create($id, Transliterator::REVERSE);
    echo $id, ' inverse ', $inv ? $inv->id : 'null', ' reverse ', $rev ? $rev->id : 'null', "\n";
}
var_dump(count(Transliterator::listIDs()), transliterator_list_ids() === Transliterator::listIDs(), in_array('Any-Latin', Transliterator::listIDs()));
$c = clone Transliterator::create('Any-Upper');
var_dump($c->id, $c->transliterate('clone'));
