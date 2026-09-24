<?php
// MessageFormatter's errors: pattern syntax errors with ICU's parse error
// position, creation failures, argument conversion failures, setPattern.
foreach (['{0', 'x}{', '{0,foo}', '{x, plural, one{a}}', '{x, select, a{a}}', '{a b}', '{00}', '{0,}', '{}',
          '{0,plural}', '{0,choice,}', '{0,choice,a#b}', '{0, plural, offset : 2 other{#}}', '{0,plural,=x{a}other{b}}',
          "{0,date,qqqqq'}", '{0,number,#.#.#}', '{0} {0,number}', '{0,number,integer} {0,number}',
          "{n,select,a{'#'} other{y}}", '{0,number,::percent}', '{0,number,::foo}', ''] as $p) {
    $m = msgfmt_create('en', $p);
    echo json_encode($p), ' => ', $m === null ? 'NULL' : 'object', ' ', intl_get_error_code(), ' ', intl_get_error_message(), "\n";
    try {
        new MessageFormatter('en', $p);
        echo "  constructed\n";
    } catch (IntlException $e) {
        echo '  ', get_class($e), ': ', $e->getMessage(), "\n";
    }
}

function f($l, $p, $a) {
    $r = MessageFormatter::formatMessage($l, $p, $a);
    echo json_encode($p), ' => ', var_export($r, true), ' ', intl_get_error_code(), ' ', intl_get_error_message(), "\n";
}
f('en', '{0,number,integer}', [2147483648]);
f('en', '{a,number,integer}', ['a' => 2147483648]);
f('en', '{0,number,integer}', [2.7]);
f('en', '{a} {a,number}', ['a' => 1]);
f('en', '{0,date}', ['abc']);
f('en', '{a,date}', ['a' => 'abc']);
f('en', '{0}', [[1, 2]]);
f('en', '{1}', [-1 => 2]);
var_dump(MessageFormatter::formatMessage('en', '{0}', ["\xff"]), intl_get_error_code());
f('en', "\xff", []);
f('en', '{0}', ['k' => [1]]);

$m = new MessageFormatter('en', 'a {0} b');
var_dump($m->setPattern('{0 a}'), $m->getErrorCode(), $m->getErrorMessage(), $m->getPattern());
var_dump($m->setPattern('{0,number}'), $m->getErrorCode(), $m->format([3]));
var_dump($m->format(['n' => 1]), $m->getErrorCode());

try {
    msgfmt_format('nope', []);
} catch (TypeError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
