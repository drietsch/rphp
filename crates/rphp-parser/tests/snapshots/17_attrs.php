<?php
#[A, B(1, x: 2), \C\D(E::F)]
#[G]
class C {
    #[P] public function __construct(#[Q] public int $x = 1, #[R] ...$rest) {}
    #[S] const K = 1;
    #[T] public $p;
}
#[U] enum E { #[V] case A; }
$f = #[W] fn() => 1;
