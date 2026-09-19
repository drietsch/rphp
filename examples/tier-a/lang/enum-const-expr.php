<?php
// An enum case's value is a constant *expression*, not a literal: php folds
// what it can and evaluates the rest in the enum's own scope.
enum Flags: int {
    const BASE = 4;
    case None = 0;
    case Read = 1 << 0;
    case Write = 1 << 1;
    case Exec = self::BASE;
    case Both = (1 << 0) | (1 << 1);
}
var_dump(Flags::Read->value, Flags::Write->value, Flags::Exec->value, Flags::Both->value);
var_dump(Flags::from(4), Flags::tryFrom(3), Flags::tryFrom(99));
var_dump(Flags::cases());
var_dump(Flags::Read instanceof BackedEnum, Flags::Read instanceof UnitEnum);

enum Names: string {
    const SUFFIX = 'x';
    case A = 'a' . self::SUFFIX;
    case B = 'b';
}
var_dump(Names::A->value, Names::A->name, Names::from('ax') === Names::A);

// A pure enum has no values to evaluate at all.
enum Pure { case One; case Two; }
var_dump(Pure::One, Pure::cases()[1]->name, Pure::One === Pure::One);
