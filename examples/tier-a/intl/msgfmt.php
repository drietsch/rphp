<?php
// MessageFormatter: simple, number, date, plural, select, nested — the
// message shapes Symfony's translator (IntlFormatter) ships.
$cases = [
    ['en', '{0} apples', [3]],
    ['en', 'Hello {name}!', ['name' => 'Ada']],
    ['en', '{0,number} / {0,number,integer} / {0,number,percent}', [1234567.891]],
    ['de', '{0,number} / {1,number,currency}', [1234567.891, 12.5]],
    ['de_DE', '{price, number, currency}', ['price' => 12.5]],
    ['fr_FR', '{n, number}', ['n' => 1234567.891]],
    ['en', '{0,number,#,##0.00;(#)}', [-3]],
    ['en', '{count, plural, =0{No apples} one{One apple} other{# apples}}', ['count' => 0]],
    ['en', '{count, plural, =0{No apples} one{One apple} other{# apples}}', ['count' => 1]],
    ['en', '{count, plural, =0{No apples} one{One apple} other{# apples}}', ['count' => 1234]],
    ['en', '{n, plural, offset:1 =0{nobody} =1{just {who}} one{{who} and # other} other{{who} and # others}}', ['n' => 3, 'who' => 'Bob']],
    ['ru', '{n, plural, one{# яблоко} few{# яблока} many{# яблок} other{# яблока}}', ['n' => 5]],
    ['ru', '{n, plural, one{# яблоко} few{# яблока} many{# яблок} other{# яблока}}', ['n' => 22]],
    ['ru', '{n, plural, one{# яблоко} few{# яблока} many{# яблок} other{# яблока}}', ['n' => 1.5]],
    ['pl', '{n, plural, one{# plik} few{# pliki} many{# plików} other{# pliku}}', ['n' => 12]],
    ['ar', '{n, plural, zero{z} one{o} two{t} few{f} many{m} other{#}}', ['n' => 103]],
    ['fr', '{n, plural, one{# chose} many{# m} other{# choses}}', ['n' => 1000000]],
    ['en', '{gender, select, male{He} female{She} other{They}} liked it', ['gender' => 'female']],
    ['en', '{gender, select, male{He} female{She} other{They}} liked it', ['gender' => 'robot']],
    ['en', '{n, selectordinal, one{#st} two{#nd} few{#rd} other{#th}}', ['n' => 23]],
    ['en', '{g, select, female{{n, plural, one{She has one} other{She has #}}} other{{n, plural, one{They have one} other{They have #}}}}', ['g' => 'female', 'n' => 4]],
    ['en', '{n, plural, other{# and {m, plural, one{# one} other{# m}}}}', ['n' => 2, 'm' => 1]],
    ['en', "It''s {0} o''clock, '{literal}' and '#'", ['five']],
    ['en', "{n, plural, one{'#' x} other{'{#}' x #}}", ['n' => 2]],
    ['en', '{0,choice,0#none|1#one|1<many {0,number}}', [5]],
    ['en', '{0,choice,0#none|1#one|1<many}', [0.5]],
    ['en', '{0} {1}', [1]],
    ['en', '{a} {b}', ['a' => 1]],
    ['en', '{0}|{0}|{0}|{0}', [true]],
    ['en', '{0}', [null]],
    ['en', '{0}', [1.5]],
    ['en', '{0} {2}', [1, 2 => 3, 5 => 6]],
    ['en', 'plain text', []],
];
foreach ($cases as [$l, $p, $a]) {
    $r = MessageFormatter::formatMessage($l, $p, $a);
    echo json_encode($p, JSON_UNESCAPED_UNICODE), ' => ';
    var_dump($r);
}

$m = new MessageFormatter('en_US', 'You have {n, plural, one{# message} other{# messages}}');
var_dump($m->format(['n' => 1]), $m->format(['n' => 7]), msgfmt_format($m, ['n' => 0]));
var_dump($m->getLocale(), $m->getPattern(), $m->getErrorCode(), $m->getErrorMessage());
var_dump($m->setPattern('{0} y'), $m->getPattern(), $m->format(['x']));
$c = clone $m;
var_dump($c->format(['z']));
$m2 = msgfmt_create('de-AT', '{0,number}');
var_dump(msgfmt_get_locale($m2), msgfmt_get_error_code($m2), msgfmt_format($m2, [1234.5]));
foreach (['de-AT', 'EN-us', 'root', 'zh-Hant-TW', 'sr-Latn-RS@x=y'] as $l) {
    echo $l, ' => ', (new MessageFormatter($l, 'x'))->getLocale(), "\n";
}
