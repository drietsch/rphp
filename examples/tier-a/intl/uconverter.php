<?php
// UConverter: the Unicode forms, US-ASCII, ISO-8859-1 and the single-byte
// tables, substitution, aliases and the registry lists.
$c = new UConverter('UTF-8', 'ISO-8859-1');
var_dump($c->getSourceEncoding(), $c->getDestinationEncoding(), $c->getSourceType(), $c->getDestinationType());
var_dump(bin2hex($c->getSubstChars()), $c->convert("caf\xe9"), bin2hex($c->convert("café", true)));
foreach (['UTF-8', 'utf8', 'ISO-8859-1', 'latin1', 'cp1252', 'ASCII', 'US-ASCII', 'UTF-16', 'UTF-16LE', 'UTF-16BE', 'UTF-32', 'UTF-32LE', 'UTF-32BE', 'KOI8-R', 'ISO-8859-15', 'iso-8859-2', 'macintosh', 'IBM037', 'ibm-00037', 'cp437', 'cp866', 'x-mac-cyrillic', 'CESU-8', 'UTF-16BE,version=1', 'UTF-16LE,version=1', 'UTF-16,version=2', 'windows-874-2000', 'tis-620', 'bogus'] as $e) {
    try {
        $u = new UConverter($e, 'UTF-8');
        echo "$e: ", $u->getDestinationEncoding(), ' type ', $u->getDestinationType(), ' subst ', bin2hex($u->getSubstChars()),
            ' conv ', bin2hex($u->convert("Aé€\u{1F600}\u{200B}ж")), ' back ', $u->convert($u->convert("Aé€ж"), true), "\n";
        $d = new UConverter('UTF-8', $e);
        $all = '';
        for ($b = 0; $b < 256; $b++) $all .= chr($b);
        echo '  decode ', bin2hex($d->convert($all)), "\n";
    } catch (Throwable $t) {
        echo "$e: ", get_class($t), ' ', $t->getMessage(), "\n";
    }
}
foreach (['Windows-1252', 'windows-1251', 'Shift_JIS'] as $e) {
    try { new UConverter($e, 'UTF-8'); } catch (IntlException $t) { echo get_class($t), ': ', $t->getMessage(), "\n"; }
}
var_dump(UConverter::transcode("caf\xe9", 'UTF-8', 'ISO-8859-1'));
var_dump(bin2hex(UConverter::transcode("€ 😀", 'ISO-8859-1', 'UTF-8')));
var_dump(bin2hex(UConverter::transcode("€ 😀", 'ISO-8859-1', 'UTF-8', ['to_subst' => '?'])));
var_dump(bin2hex(UConverter::transcode("a\xffb", 'UTF-16LE', 'UTF-8', ['from_subst' => '?'])));
var_dump(bin2hex(UConverter::transcode("a\x80b\xe2\x82", 'UTF-16BE', 'UTF-8')));
var_dump(bin2hex(UConverter::transcode("a\xed\xa0\x80b", 'UTF-16BE', 'UTF-8')));
var_dump(bin2hex(UConverter::transcode("\x00a\xd8\x00\x00b\xdc", 'UTF-8', 'UTF-16BE')));
var_dump(bin2hex(UConverter::transcode("a\x00\x00\x00\x00\x00\x11\x00b", 'UTF-8', 'UTF-32LE')));
var_dump(bin2hex(UConverter::transcode("\xfe\xff\x00a\xff\xfe", 'UTF-8', 'UTF-16')));
var_dump(bin2hex(UConverter::transcode("\xff\xfea\x00", 'UTF-8', 'UTF-16')));
var_dump(bin2hex(UConverter::transcode("", 'UTF-16', 'UTF-8')));
var_dump(UConverter::transcode('x', 'bogus', 'UTF-8'), intl_get_error_code(), intl_get_error_message());
var_dump(UConverter::transcode('x', 'ASCII', 'UTF-8', ['to_subst' => '??']), intl_get_error_code(), intl_get_error_message());
for ($r = 0; $r <= 5; $r++) var_dump(UConverter::reasonText($r));
try { UConverter::reasonText(99); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
var_dump(count(UConverter::getAvailable()), array_slice(UConverter::getAvailable(), 0, 8));
var_dump(UConverter::getAliases('latin1'), UConverter::getAliases('UTF16_PlatformEndian'), UConverter::getAliases('bogus'));
var_dump(UConverter::getStandards());
$c = new UConverter('ASCII', 'UTF-8');
var_dump($c->setSubstChars('!'), bin2hex($c->getSubstChars()), $c->convert("aéb"));
var_dump($c->setSubstChars('!!!!!'), $c->getErrorCode(), $c->getErrorMessage());
var_dump($c->setSubstChars(''), $c->getErrorCode(), $c->getErrorMessage());
var_dump($c->setSourceEncoding('latin1'), $c->getSourceEncoding(), $c->setDestinationEncoding('bogus'), $c->getErrorCode(), $c->getErrorMessage(), $c->getDestinationEncoding());
var_dump($c->setDestinationEncoding('windows-1252'), intl_get_error_code(), intl_get_error_message(), $c->getDestinationEncoding());
$d = new UConverter();
var_dump($d->getSourceEncoding(), $d->getDestinationEncoding(), $d->convert("abc"));
$e = clone $c;
var_dump($e->getSourceEncoding(), $e->convert("\xe9"));
try { serialize($c); } catch (Exception $x) { echo $x->getMessage(), "\n"; }
