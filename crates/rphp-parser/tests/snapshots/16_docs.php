<?php
/** doc1 */ #[A] function f() {}
#[A] /** doc2 */ function g() {}
/** doc3 */

// c
class C { /** p */ public $p; /** m */ #[X] public function m() {} /** k */ const K = 1; /** t */ use T; }
/** e */ enum E { /** case */ case A; }
/** cl */ $c = function() {}; /** af */ $d = fn() => 1;
/** none */ echo 1; function h() {}
