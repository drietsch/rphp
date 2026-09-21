<?php
// array_column over objects reads public properties: enum cases' name/value, a column and an index key.
enum Kind: string { case A = "a"; case B = "b";
  public static function values(): array { return array_column(static::cases(), "value"); } }
var_dump(array_column(Kind::cases(), "value"));
var_dump(Kind::values());
var_dump(array_column(Kind::cases(), "name"));
var_dump(array_column([Kind::A, Kind::B], "value", "name"));
