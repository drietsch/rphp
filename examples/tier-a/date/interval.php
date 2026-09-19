<?php
// DateInterval: the ISO 8601 duration grammar, the property set php shows,
// format()'s specifiers, and diff()'s decomposition. Fixed dates only: the
// differential suite compares byte for byte.
date_default_timezone_set('UTC');

// A parsed duration shows ten keys; createFromDateString shows two.
var_dump(new DateInterval('P1D'));
var_dump(DateInterval::createFromDateString('2 days + 3 hours'));
var_dump(get_object_vars(new DateInterval('P1Y2M3DT4H5M6S')));

// The grammar, including the week form and the two combined spellings.
foreach (['P1Y', 'P1M', 'P1W', 'P10D', 'PT1H', 'PT1M', 'PT30S',
          'P1Y2M3W4DT5H6M7S', 'P2W3D', 'P0003-06-04T12:30:05',
          'P2021-01-02T03:04:05'] as $spec) {
    $i = new DateInterval($spec);
    printf("%-22s y=%d m=%d d=%d h=%d i=%d s=%d invert=%d\n",
        $spec, $i->y, $i->m, $i->d, $i->h, $i->i, $i->s, $i->invert);
}
foreach (['', 'P', 'P1X', 'PT1.5S', 'PT1,5S', 'P1.5Y', 'P0003-06-04', '--',
          'P-1D', 'bogus', 'P00030604T123005', 'P0003-06-04T12:30'] as $spec) {
    try {
        new DateInterval($spec);
        echo "no throw: $spec\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}

// format()'s whole set. `%a` is "(unknown)" for anything diff() did not make,
// an unknown specifier passes through, and a trailing `%` disappears.
$i = new DateInterval('P1Y2M3DT4H5M6S');
$i->f = 0.25;
echo $i->format('%%|%Y|%y|%M|%m|%D|%d|%H|%h|%I|%i|%S|%s|%F|%f|%R|%r|%a'), "\n";
$i->f = 0.000005;
echo $i->format('%F|%f'), "\n";
echo $i->format('%Q|%'), "\n";
echo $i->format('a%%b'), "\n";
$inv = new DateInterval('P1D');
$inv->invert = 1;
echo $inv->format('%R|%r|%d'), "\n";

// A field is real state: writing it changes what format() and add() see.
$w = new DateInterval('P1D');
$w->d = 5;
echo $w->format('%d'), ' ', (new DateTimeImmutable('2021-01-01'))->add($w)->format('Y-m-d'), "\n";

// diff(): the y/m/d decomposition is a field-wise subtraction with timelib's
// borrow rules, so January 31 to March 1 is 29 days and not one month one day.
foreach ([
    ['2021-01-01', '2023-05-06'],
    ['2021-01-31', '2021-03-01'],
    ['2021-01-31', '2021-02-28'],
    ['2020-02-29', '2021-02-28'],
    ['2021-03-01', '2021-01-31'],
    ['2021-01-01 23:00', '2021-01-02 01:00'],
    ['2021-01-01', '2021-01-01'],
] as [$a, $b]) {
    $r = (new DateTime($a))->diff(new DateTime($b));
    printf("%-20s -> %-20s y=%d m=%d d=%d h=%d i=%d s=%d days=%s invert=%d\n",
        $a, $b, $r->y, $r->m, $r->d, $r->h, $r->i, $r->s, var_export($r->days, true), $r->invert);
    echo '   ', $r->format('%R %y-%m-%d %h:%i:%s (%a days)'), "\n";
}

// diff() across a DST transition, in one zone and between two.
$berlin = new DateTimeZone('Europe/Berlin');
$ny = new DateTimeZone('America/New_York');
foreach ([
    ['2021-03-27 12:00', $berlin, '2021-03-28 12:00', $berlin],
    ['2021-03-27 12:00', $berlin, '2021-03-28 11:00', $berlin],
    ['2021-03-27 12:00', $berlin, '2021-03-28 13:00', $berlin],
    ['2021-10-30 12:00', $berlin, '2021-10-31 12:00', $berlin],
    ['2021-10-30 12:00', $berlin, '2021-10-31 11:00', $berlin],
    ['2021-01-01 00:00', new DateTimeZone('UTC'), '2021-01-02 00:00', $ny],
    ['2021-11-06 12:00', $ny, '2021-11-08 12:00', $ny],
] as [$a, $az, $b, $bz]) {
    $x = new DateTime($a, $az);
    $y = new DateTime($b, $bz);
    $r = $x->diff($y);
    printf("%s %-16s -> %s %-16s d=%d h=%d i=%d days=%d inv=%d elapsed=%d\n",
        $a, $az->getName(), $b, $bz->getName(), $r->d, $r->h, $r->i, $r->days,
        $r->invert, $y->getTimestamp() - $x->getTimestamp());
}

// $absolute drops the sign.
$back = (new DateTime('2023-05-06'))->diff(new DateTime('2021-01-01'), true);
printf("absolute: y=%d m=%d d=%d days=%d invert=%d\n", $back->y, $back->m, $back->d, $back->days, $back->invert);

// Microseconds survive the round trip.
$us = (new DateTime('2021-01-01 00:00:00.750000'))->diff(new DateTime('2021-01-01 00:00:01.250000'));
printf("us: s=%d f=%s days=%s\n", $us->s, var_export($us->f, true), var_export($us->days, true));

// add()/sub() with a createFromDateString interval apply php's *relative*
// struct and nothing else: a weekday walk survives and the clock is kept,
// but `first day of` and an absolute time do not, so add() and modify() of
// the same string disagree.
$base = new DateTimeImmutable('2021-06-15 10:20:30');
foreach (['next monday', 'first day of next month', 'last day of this month',
          'first monday of next month', '+2 weekdays', '3 days ago',
          '+1 month 2 days', 'midnight', '2 weeks'] as $spec) {
    printf("%-28s add=%s modify=%s\n", $spec,
        $base->add(DateInterval::createFromDateString($spec))->format('Y-m-d H:i:s'),
        $base->modify($spec)->format('Y-m-d H:i:s'));
}
echo $base->sub(DateInterval::createFromDateString('2 weeks'))->format('Y-m-d'), "\n";
echo $base->add($inv)->format('Y-m-d'), "\n";
echo $base->sub($inv)->format('Y-m-d'), "\n";

// Serialization.
var_dump((new DateInterval('P1D'))->__serialize());
var_dump(DateInterval::createFromDateString('2 days')->__serialize());
echo serialize(new DateInterval('P1D')), "\n";
var_export(new DateInterval('P1D'));
echo "\n";
var_export(DateInterval::createFromDateString('2 days'));
echo "\n";
$round = unserialize(serialize(new DateInterval('P1Y2M3DT4H5M6S')));
echo $round->format('%y-%m-%d %h:%i:%s'), "\n";
var_dump(DateInterval::__set_state(['y' => 1, 'm' => 2, 'd' => 3, 'h' => 4,
    'i' => 5, 's' => 6, 'f' => 0.0, 'invert' => 0, 'days' => false,
    'from_string' => false]));

// The procedural aliases.
echo date_interval_format(date_interval_create_from_date_string('2 days'), '%d'), "\n";
$d = date_diff(date_create('2021-01-01'), date_create('2021-03-15'));
echo date_interval_format($d, '%y %m %d %a'), "\n";
echo date_format(date_add(date_create('2021-01-01'), new DateInterval('P1M')), 'Y-m-d'), "\n";
echo date_format(date_sub(date_create('2021-03-31'), new DateInterval('P1M')), 'Y-m-d'), "\n";

// A `createFromDateString` interval shows only the string, but php answers
// the whole struct behind it: the fields are the *relative* amounts, `ago`
// lands in the sign rather than in `invert`, a weekday walk is no number of
// days, and `days` stays false.
foreach (['2 days', '2 days 3 hours ago', 'next monday', '2 weeks',
          '+1 month 2 days', 'tomorrow', 'midnight', 'now'] as $spec) {
    $s = DateInterval::createFromDateString($spec);
    printf("%-20s y=%d m=%d d=%d h=%d i=%d s=%d invert=%d days=%s fmt=[%s] isset=%d%d\n",
        $spec, $s->y, $s->m, $s->d, $s->h, $s->i, $s->s, $s->invert,
        var_export($s->days, true), $s->format('%y %m %d %h %R %a'),
        (int) isset($s->d), (int) isset($s->nope));
}
var_dump((new DateInterval('P1D'))->nope);

// The string must be purely relative, and the interval half of the
// extension reports a bad one in its own words.
foreach (['1.5 hours', 'bogus', '', '@@', '1 junk', '2021-01-01', '10:00',
          '2 days UTC', '@1600000000'] as $bad) {
    try {
        DateInterval::createFromDateString($bad);
        echo "no throw: $bad\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
var_dump(date_interval_create_from_date_string('bogus'));
var_dump(date_interval_create_from_date_string('1 junk'));
