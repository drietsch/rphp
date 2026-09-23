<?php
// Tier-A differential: BcMath\Number's methods and their automatic result
// scales — add/sub keep the larger scale, mul adds them, div/pow(-n)/sqrt
// compute ten more digits and trim, mod keeps the larger, divmod's
// quotient is an integer — against an explicit $scale; the argument
// forms (Number, int, numeric string, float, bool, null, Stringable) and
// the errors.

use BcMath\Number;

function show(string $label, Number $r): void
{
    echo str_pad($label, 28), $r->value, " (scale ", $r->scale, ")\n";
}

$a = new Number("12.500");
$b = new Number("-3.25");
show("add", $a->add($b));
show("add int", $a->add(7));
show("add str", $a->add("0.0001"));
show("add scale 1", $a->add("0.0999", 1));
show("sub", $a->sub($b));
show("sub scale 0", $a->sub("0.9", 0));
show("mul", $a->mul($b));
show("mul scale 2", $a->mul($b, 2));
show("div", $a->div($b));
show("div 3", (new Number(1))->div(3));
show("div exact", (new Number("10"))->div(4));
show("div scale 3", $a->div(7, 3));
show("mod", $a->mod($b));
show("mod scale 0", $a->mod("0.3", 0));
show("pow 2", $b->pow(2));
show("pow 3", $b->pow(3));
show("pow 0", $b->pow(0));
show("pow -2", $b->pow(-2));
show("pow -1 scale 3", $a->pow(-1, 3));
show("pow Number", $a->pow(new Number(2)));
show("sqrt", $a->sqrt());
show("sqrt 2", (new Number(2))->sqrt());
show("sqrt scale 5", (new Number(2))->sqrt(5));
show("sqrt 0.25", (new Number("0.25"))->sqrt());
show("floor", $b->floor());
show("ceil", $b->ceil());
show("round", $a->round());
show("round 1", (new Number("1.25"))->round(1));
show("round half even", (new Number("1.25"))->round(1, RoundingMode::HalfEven));
show("round -1", (new Number("125"))->round(-1, RoundingMode::HalfTowardsZero));
show("round 5", (new Number("1.25"))->round(5));
show("powmod", (new Number(4))->powmod(13, 497));
show("powmod scale", (new Number(4))->powmod("13", new Number(497), 2));
[$q, $r] = $a->divmod($b);
show("divmod q", $q);
show("divmod r", $r);
[$q, $r] = $a->divmod("0.3", 5);
show("divmod scale q", $q);
show("divmod scale r", $r);
var_dump($a->compare($b), $a->compare("12.5"), $a->compare(13), $b->compare("-3.2500001", 5), $b->compare("-3.2500001"));

// Weak-mode argument forms.
class Two { public function __toString(): string { return "2.0"; } }
show("bool", $a->add(true));
show("float whole", $a->add(2.0));
show("float frac", $a->add(2.5));
show("stringable", $a->add(new Two));
show("null", $a->add(null));

$errors = [
    fn() => $a->add([]),
    fn() => $a->add(new stdClass),
    fn() => $a->add("1e3"),
    fn() => $a->add(1e30),
    fn() => $a->add(INF),
    fn() => $a->add(1, -1),
    fn() => $a->add(1, new stdClass),
    fn() => $a->div(0),
    fn() => $a->div("0.00"),
    fn() => $a->mod(0),
    fn() => $a->divmod(0),
    fn() => $a->pow("0.5"),
    fn() => $a->pow("99999999999999999999"),
    fn() => (new Number(0))->pow(-1),
    fn() => $a->powmod(2, 3),
    fn() => (new Number(4))->powmod("2.5", 3),
    fn() => (new Number(4))->powmod(-2, 3),
    fn() => (new Number(4))->powmod(2, "3.5"),
    fn() => (new Number(4))->powmod(2, 0),
    fn() => (new Number(-4))->sqrt(),
    fn() => $a->sqrt(-1),
    fn() => $a->round(0, 3),
    fn() => $a->round(2147483648),
    fn() => $a->compare("x"),
    fn() => $a->add(),
];
foreach ($errors as $f) {
    try {
        $r = $f();
        echo $r, "\n";
    } catch (\Throwable $t) {
        echo get_class($t), ": ", $t->getMessage(), "\n";
    }
}
