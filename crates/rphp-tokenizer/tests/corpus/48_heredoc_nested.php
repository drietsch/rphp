<?php
$a = <<<OUT
before {$x[<<<IN
inner $y
IN]} after
{${<<<'NOW'
nowdoc
NOW}}
  OUT;
$b = <<<A
{$f(<<<B
  b
  B, <<<'C'
c
C)}
A;
$c = "{$x[<<<H
h
H]}";
$d = <<<OUT
{$x[<<<IN
    in
    IN]}
  OUT;
$e = <<<OUT
  {$x[<<<IN
  in
  IN]}
  OUT;
$f = <<<OUT
{$x[<<<'IN'
    in
    IN]}
  OUT;
$g = `{$x[<<<H
h
  H]}`;
$h = 1;
