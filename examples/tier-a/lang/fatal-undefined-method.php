<?php
// Uncaught Error: Call to undefined method A::nope() — both engines exit 255.
class A {}
$a = new A();
$a->nope();
