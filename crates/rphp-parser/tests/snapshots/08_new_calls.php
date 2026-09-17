<?php
new A()->b; new A()->b(); new (f()); new $a->b(); new static; new A; new A(1)[0]; new A()::B;
static::x(); X::class; $x::class; X::{$e}; X::$p; X::$$p; self::CONST; parent::m(); $a::$b(); $a::{$m}();
$o->list(); $o->class; C::new(); $o?->fn; $o?->m()->n?->p; f(name: 1, class: 2);
clone $a; clone($a); clone($a, ["x" => 1]); clone(object: $a, withProperties: []);
$x |> strlen(...) |> (fn($n) => $n + 1);
