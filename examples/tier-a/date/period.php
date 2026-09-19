<?php
// DatePeriod: both constructor forms, the ISO 8601 factory, the boundary
// flags and iteration. Fixed dates only: the differential suite compares
// byte for byte.
date_default_timezone_set('UTC');

// The seven properties php shows. `recurrences` is the count plus the
// boundary flags, which is exactly how many dates the loop yields.
var_dump(new DatePeriod(new DateTime('2021-01-01'), new DateInterval('P1D'), 2));
var_dump(new DatePeriod(new DateTime('2021-01-01'), new DateInterval('P1D'), new DateTime('2021-01-04')));

// The recurrence form: the argument, the stored count and the yield.
foreach ([1, 2, 3] as $n) {
    $p = new DatePeriod(new DateTime('2021-01-01'), new DateInterval('P1D'), $n);
    echo "n=$n prop=", $p->recurrences, ' get=', $p->getRecurrences(),
        ' count=', iterator_count($p), "\n";
}

// Iteration, with each of the two boundary flags.
function show(string $label, DatePeriod $p): void
{
    echo $label, ':';
    foreach ($p as $k => $v) {
        echo ' ', $k, '=>', $v->format('Y-m-d');
    }
    echo "\n";
}
$start = new DateTime('2021-01-01');
$day = new DateInterval('P1D');
show('end', new DatePeriod($start, $day, new DateTime('2021-01-04')));
show('end+incl', new DatePeriod($start, $day, new DateTime('2021-01-04'), DatePeriod::INCLUDE_END_DATE));
show('rec', new DatePeriod($start, $day, 2));
show('rec-excl', new DatePeriod($start, $day, 2, DatePeriod::EXCLUDE_START_DATE));
show('both', new DatePeriod($start, $day, new DateTime('2021-01-04'),
    DatePeriod::EXCLUDE_START_DATE | DatePeriod::INCLUDE_END_DATE));

// The yielded objects have the class of the start date.
foreach (new DatePeriod(new DateTimeImmutable('2021-01-01'), $day, 1) as $v) {
    echo get_class($v), ' ', $v->format('Y-m-d'), "\n";
}

// A month step lands on php's month arithmetic, not on "the same day".
show('months', new DatePeriod(new DateTime('2021-01-31'), new DateInterval('P1M'), 3));
show('weeks', new DatePeriod(new DateTime('2021-01-01'), new DateInterval('P1W'), 3));

// A period steps by wall clock, so it keeps 12:00 across a spring-forward
// even when the interval is PT24H — unlike DateTime::add(), which would
// move that one to 13:00.
$berlin = new DateTimeZone('Europe/Berlin');
foreach (['P1D', 'PT24H'] as $spec) {
    echo $spec, ':';
    foreach (new DatePeriod(new DateTime('2021-03-27 12:00:00', $berlin), new DateInterval($spec), 2) as $v) {
        echo ' ', $v->format('Y-m-d H:i T');
    }
    echo "\n";
}
// And across a fall-back in America/New_York.
echo 'fall:';
foreach (new DatePeriod(new DateTime('2021-11-06 12:00:00', new DateTimeZone('America/New_York')), $day, 2) as $v) {
    echo ' ', $v->format('Y-m-d H:i T');
}
echo "\n";

// The getters. getRecurrences() is null for the end-date form.
$p = new DatePeriod($start, $day, 3);
echo $p->getStartDate()->format('Y-m-d'), ' ',
    var_export($p->getEndDate(), true), ' ',
    $p->getDateInterval()->format('%d'), ' ',
    var_export($p->getRecurrences(), true), "\n";
$q = new DatePeriod($start, $day, new DateTime('2021-01-05'));
echo $q->getStartDate()->format('Y-m-d'), ' ',
    $q->getEndDate()->format('Y-m-d'), ' ',
    var_export($q->getRecurrences(), true), "\n";

// createFromISO8601String.
$iso = DatePeriod::createFromISO8601String('R4/2012-07-01T00:00:00Z/P7D');
echo get_class($iso->getStartDate()), ' ', $iso->getStartDate()->format('c e'), ' ',
    $iso->getRecurrences(), ' ', $iso->recurrences, "\n";
foreach ($iso as $v) {
    echo '  ', $v->format('c'), "\n";
}
$iso2 = DatePeriod::createFromISO8601String('R3/2008-03-01T13:00:00Z/P1Y2M10DT2H30M');
echo $iso2->getDateInterval()->format('%y-%m-%d %h:%i:%s'), "\n";
foreach ($iso2 as $v) {
    echo '  ', $v->format('c'), "\n";
}
// php's scanner reads at most nine recurrence digits.
$big = DatePeriod::createFromISO8601String('R2147483640/2012-07-01T00:00:00Z/P7D');
echo $big->getRecurrences(), ' ', $big->recurrences, "\n";
echo DatePeriod::createFromISO8601String('R2/20210101T000000Z/P1D')->getStartDate()->format('c e'), "\n";

// Serialization.
var_dump((new DatePeriod($start, $day, 1))->__serialize());
var_export(new DatePeriod($start, $day, 2));
echo "\n";

// The error paths.
foreach ([
    fn() => new DatePeriod(new DateTime('2021-01-01'), new DateInterval('P1D'), 0),
    fn() => new DatePeriod(new DateTime('2021-01-01'), new DateInterval('P1D'), -1),
    fn() => new DatePeriod(new DateTime('2021-01-01'), new DateInterval('P1D')),
    fn() => DatePeriod::createFromISO8601String('bogus'),
    fn() => DatePeriod::createFromISO8601String('R/2012-07-01T00:00:00Z/P7D'),
    fn() => DatePeriod::createFromISO8601String('2012-07-01T00:00:00Z/P7D'),
    fn() => DatePeriod::createFromISO8601String('2008-03-01T13:00:00Z/2008-05-11T15:30:00Z'),
    fn() => DatePeriod::createFromISO8601String('R2/2021-01-01T00:00:00+02:00/P1D'),
] as $probe) {
    try {
        $probe();
        echo "no throw\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}

// The ISO scanner classifies every `/`-separated component on its own —
// `R<n>`, an instant, a duration — so the count may come anywhere and an
// empty component is ignored. Only a date met *before* a duration is the
// start, which is what makes `R2/P7D/<date>` the "duration then end date"
// form and therefore startless.
foreach (['2008-03-01T13:00:00Z/R2/P7D', 'R2/2008-03-01T13:00:00Z/P7D/',
          'R000002/20080301T130000Z/P7D', 'R99999999999/2008-03-01T13:00:00Z/P7W'] as $ok) {
    $p = DatePeriod::createFromISO8601String($ok);
    echo $ok, ' => ', $p->getStartDate()->format('c'), ' d=',
        $p->getDateInterval()->format('%d'), ' rec=', $p->recurrences, "\n";
}

// The three semantic errors come in php's order: start date, interval,
// recurrence count.
foreach (['P7D', 'R2/P7D', 'R2/P7D/2008-03-01T13:00:00Z', 'R2/2008-03-01T13:00:00Z',
          '2008-03-01T13:00:00Z/2008-05-11T15:30:00Z', 'R0/2008-03-01T13:00:00Z/P7D',
          'R2147483640/2008-03-01T13:00:00Z/2008-05-11T15:30:00Z', '', 'Pbogus',
          'r2/2008-03-01T13:00:00Z/P7D', 'R2/2008-03-01T13:00:00Z/P',
          '2008-03-01T13:00:00Z/P7D/P1D'] as $bad) {
    try {
        DatePeriod::createFromISO8601String($bad);
        echo "no throw: $bad\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
