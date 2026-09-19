<?php
// DateTime, DateTimeImmutable and DateTimeZone. Fixed dates only: the
// differential suite compares byte for byte.
date_default_timezone_set('UTC');

// The three keys php shows, for all three timezone_type shapes.
var_dump(new DateTime('2021-06-15 10:20:30.123456'));
var_dump(new DateTimeImmutable('2021-06-15 10:20:30', new DateTimeZone('Europe/Berlin')));
var_dump(new DateTimeZone('Europe/Berlin'));
var_dump(new DateTimeZone('+05:30'));
var_dump(new DateTimeZone('CEST'));

// A string with its own zone beats the constructor argument, and `@N` is
// always the fixed offset +00:00.
echo (new DateTime('2021-01-01 00:00:00 UTC', new DateTimeZone('Europe/Berlin')))->format('c e'), "\n";
echo (new DateTime('@1000000000', new DateTimeZone('Europe/Berlin')))->format('c e'), "\n";
echo (new DateTime('2021-01-01', new DateTimeZone('Europe/Berlin')))->format('c e'), "\n";

// The whole point of the two classes.
$m = new DateTime('2021-01-31 12:00:00');
$r = $m->modify('+1 month');
echo 'mutable: ', $m->format('Y-m-d H:i:s'), ' same=', var_export($m === $r, true), "\n";
$i = new DateTimeImmutable('2021-01-31 12:00:00');
$j = $i->modify('+1 month');
echo 'immutable: ', $i->format('Y-m-d H:i:s'), ' -> ', $j->format('Y-m-d H:i:s'),
    ' same=', var_export($i === $j, true), "\n";

// php has two additions. add() moves the calendar civilly (P1D is the same
// wall clock the next day, and costs 23 real hours across a spring-forward)
// but the clock absolutely (PT24H really is 86400 seconds), while modify()
// is civil throughout.
$berlin = new DateTimeZone('Europe/Berlin');
$dst = new DateTimeImmutable('2021-03-27 12:00:00', $berlin);
echo 'modify +24 hours: ', $dst->modify('+24 hours')->format('Y-m-d H:i:s T P'), "\n";
foreach (['P1D', 'PT24H', 'PT23H', 'P1DT1H'] as $spec) {
    $next = $dst->add(new DateInterval($spec));
    echo str_pad($spec, 6), $next->format('Y-m-d H:i:s T P'),
        ' elapsed=', $next->getTimestamp() - $dst->getTimestamp(), "\n";
}
// And back across a fall-back in America/New_York.
$ny = new DateTimeImmutable('2021-11-06 12:00:00', new DateTimeZone('America/New_York'));
echo $ny->add(new DateInterval('P1D'))->format('Y-m-d H:i:s T P'), "\n";
echo $ny->sub(new DateInterval('P1D'))->format('Y-m-d H:i:s T P'), "\n";

// setTimezone moves the wall clock, not the instant.
$fixed = new DateTimeImmutable('2021-06-15 10:20:30.123456', $berlin);
echo $fixed->getTimestamp(), ' ', $fixed->getOffset(), ' ', $fixed->getMicrosecond(), "\n";
echo $fixed->setTimezone(new DateTimeZone('America/New_York'))->format('Y-m-d H:i:s.u T P'), "\n";
echo $fixed->setTimezone(new DateTimeZone('UTC'))->format('Y-m-d H:i:s.u T P'), "\n";
echo $fixed->getTimestamp() === $fixed->setTimezone(new DateTimeZone('UTC'))->getTimestamp() ? "same instant\n" : "moved\n";

// The setters normalize the way mktime() does.
$s = new DateTimeImmutable('2021-06-15 10:20:30.123456');
echo $s->setDate(2020, 14, 40)->format('Y-m-d H:i:s.u'), "\n";
echo $s->setISODate(2021, 1, 1)->format('Y-m-d'), ' ',
    $s->setISODate(2021, 53, 7)->format('Y-m-d'), ' ',
    $s->setISODate(2021, 1)->format('Y-m-d'), "\n";
echo $s->setTime(25, 70, 90, 1234567)->format('Y-m-d H:i:s.u'), "\n";
echo $s->setTimestamp(0)->format('Y-m-d H:i:s.u e'), "\n";
echo $s->setMicrosecond(7)->format('Y-m-d H:i:s.u'), "\n";

// The class constants.
$c = new DateTimeImmutable('2009-02-13 23:31:30', $berlin);
// (RFC7231 is left out: reading it raises an 8.5 deprecation of its own.)
foreach (['ATOM', 'COOKIE', 'ISO8601', 'ISO8601_EXPANDED', 'RFC822', 'RFC850',
          'RFC1036', 'RFC1123', 'RFC2822', 'RFC3339',
          'RFC3339_EXTENDED', 'RSS', 'W3C'] as $name) {
    echo str_pad($name, 18), $c->format(constant('DateTimeInterface::' . $name)), "\n";
}

// createFromFormat: `!` resets everything, `|` resets the rest, `+` forgives
// trailing data.
foreach ([
    ['!Y-m-d', '2021-02-03'],
    ['Y-m-d|', '2021-02-03'],
    ['!d/m/Y H:i:s', '03/02/2021 04:05:06'],
    ['!Y-m-d\TH:i:sP', '2021-02-03T04:05:06+02:00'],
    ['U', '1000000000'],
    ['U.u', '1000000000.123456'],
    ['!Y-m-d H:i:s.u', '2021-02-03 04:05:06.789012'],
    ['!D, d M Y H:i:s T', 'Wed, 03 Feb 2021 04:05:06 CET'],
    ['!y-n-j g:i a', '21-2-3 4:05 pm'],
    ['!Y-m-d e', '2021-02-03 Europe/Berlin'],
    ['!S j M Y', 'rd 3 Feb 2021'],
    ['!Y-m-d+', '2021-02-03 junk'],
] as [$fmt, $text]) {
    $d = DateTime::createFromFormat($fmt, $text);
    echo str_pad($fmt, 20), ' ', $d === false ? 'false' : $d->format('Y-m-d H:i:s.u e P'), "\n";
}
var_dump(DateTime::createFromFormat('!Y-m-d', '2021-02-03 junk'));
var_dump(DateTime::getLastErrors());

// The converting factories.
echo get_class(DateTime::createFromImmutable(new DateTimeImmutable('2021-01-01', $berlin))), ' ',
    DateTime::createFromImmutable(new DateTimeImmutable('2021-01-01', $berlin))->format('c e'), "\n";
echo get_class(DateTimeImmutable::createFromMutable(new DateTime('2021-01-01'))), "\n";
echo get_class(DateTimeImmutable::createFromInterface(new DateTime('2021-01-01'))), "\n";
echo DateTime::createFromTimestamp(1000000000)->format('c e'), "\n";
echo DateTime::createFromTimestamp(-1.25)->format('Y-m-d H:i:s.u e'), "\n";

// Serialization.
$x = new DateTime('2021-01-01 00:00:00', $berlin);
var_dump($x->__serialize());
echo serialize($x), "\n";
var_export($x);
echo "\n";
echo unserialize(serialize($x))->format('c e'), "\n";
echo DateTime::__set_state(['date' => '2021-01-01 00:00:00.000000', 'timezone_type' => 3, 'timezone' => 'Europe/Berlin'])->format('c e'), "\n";
var_dump((new DateTimeZone('Europe/Berlin'))->__serialize());
echo serialize(new DateTimeZone('CEST')), "\n";

// DateTimeZone's own surface.
$z = new DateTimeZone('Europe/Berlin');
echo $z->getName(), ' ', $z->getOffset(new DateTime('@1609459200')), ' ',
    $z->getOffset(new DateTime('@1625097600')), "\n";
var_dump($z->getTransitions(1616000000, 1620000000));
var_dump((new DateTimeZone('+02:00'))->getTransitions(0, 100));
var_dump(DateTimeZone::listIdentifiers(DateTimeZone::PER_COUNTRY, 'AT'));
echo count(DateTimeZone::listIdentifiers()), ' ',
    count(DateTimeZone::listIdentifiers(DateTimeZone::ALL_WITH_BC)), ' ',
    count(DateTimeZone::listIdentifiers(DateTimeZone::EUROPE)), "\n";
$abbr = DateTimeZone::listAbbreviations();
var_dump(count($abbr), $abbr['cest'][0]);

// clone keeps the hidden instant.
$orig = new DateTime('2021-06-15 10:20:30.123456', $berlin);
$copy = clone $orig;
$copy->modify('+1 year');
echo $orig->format('c'), ' ', $copy->format('c'), "\n";

// The procedural aliases.
echo date_format(date_create('2021-06-15 10:20:30', timezone_open('Europe/Berlin')), 'c e'), "\n";
echo date_format(date_modify(date_create('2021-01-31'), '+1 month'), 'Y-m-d'), "\n";
echo date_timestamp_get(date_create('@1000000000')), ' ',
    date_offset_get(date_create('2021-06-15', timezone_open('Europe/Berlin'))), "\n";
echo timezone_name_get(date_timezone_get(date_create('2021-01-01', timezone_open('Asia/Tokyo')))), "\n";
echo date_format(date_date_set(date_create('2021-01-01 05:06:07'), 2022, 3, 4), 'c'), "\n";
echo date_format(date_isodate_set(date_create('2021-01-01'), 2021, 10, 3), 'Y-m-d'), "\n";
echo date_format(date_time_set(date_create('2021-01-01'), 1, 2, 3), 'c'), "\n";
echo date_format(date_timestamp_set(date_create('2021-01-01'), 12345), 'c'), "\n";
echo date_format(date_create_from_format('!Y-m-d', '2021-02-03'), 'c'), "\n";
echo get_class(date_create_immutable_from_format('!Y-m-d', '2021-02-03')), "\n";
echo timezone_offset_get(timezone_open('Europe/Berlin'), date_create('@1625097600')), "\n";
echo timezone_name_from_abbr('CEST'), ' ', timezone_name_from_abbr('', 3600, 0), "\n";
var_dump(date_create('bogus input'));
var_dump(timezone_open('Nowhere/Nothing'));

// The error paths.
foreach ([
    fn() => new DateTime('bogus input'),
    fn() => new DateTime('2021-13-45'),
    fn() => new DateTimeZone('Nowhere/Nothing'),
    fn() => new DateTimeZone(''),
    fn() => (new DateTime('2021-01-01'))->modify('bogus'),
    fn() => (new DateTimeImmutable('2021-01-01'))->modify('bogus'),
    fn() => (new DateTime('2021-01-01'))->setMicrosecond(1000000),
    fn() => new DateTimeInterface(),
] as $probe) {
    try {
        $probe();
        echo "no throw\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}

// The exception tree.
foreach (['DateError', 'DateObjectError', 'DateRangeError', 'DateException',
          'DateInvalidTimeZoneException', 'DateInvalidOperationException',
          'DateMalformedStringException', 'DateMalformedIntervalStringException',
          'DateMalformedPeriodStringException'] as $class) {
    echo str_pad($class, 38), get_parent_class($class), "\n";
}
