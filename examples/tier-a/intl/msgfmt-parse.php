<?php
// MessageFormatter::parse() / parseMessage(): literal text, strings,
// numbers, dates and choices; what ICU cannot parse.
function t($l, $p, $s) {
    $r = MessageFormatter::parseMessage($l, $p, $s);
    echo json_encode($p), ' <- ', json_encode($s), ' => ', var_export($r, true), "\n";
    if ($r === false) {
        echo '   ', intl_get_error_code(), ' ', intl_get_error_message(), "\n";
    }
}
t('en', '{0} apples', '3 apples');
t('en', '{0,number} apples', '1,234.5 apples');
t('en', '{0,number,integer} x', '1,234 x');
t('en', '{0,number} and {1,number}', '3 and 4.5');
t('de', '{0,number} x', '1.234,5 x');
t('en', '{1} and {0}', 'a and b');
t('en', '{0}', '');
t('en', 'x{0}', 'x');
t('en', '{0} y', 'z y trailing');
t('en', 'abc', 'abd');
t('en', '{0,number,percent}', '25%');
t('en', '{0,date}', 'Jan 2, 2020');
t('en', '{0,choice,0#none|1#one|1<many}', 'many');
t('en', '{0,choice,0#none|1#one|1<many}', 'zzz');
t('en', '{n}', 'x');
t('en', '{0,plural,other{#}}', '3');
t('en', '{0} {1}', '{0} b');
t('en', '{0,number}', 'abc');
t('en', "It''s {0}", "It's me");
t('en', '{0,number}', '-5');
t('en', '{0,number}', '99999999999999999999');
$m = new MessageFormatter('en', '{0,number} and {1}');
var_dump($m->parse('7 and z'), $m->getErrorCode());
var_dump($m->parse('x'), $m->getErrorCode(), $m->getErrorMessage());
var_dump(msgfmt_parse($m, '1 and 2'));
