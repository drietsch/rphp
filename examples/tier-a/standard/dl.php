<?php
// Only the failure path php takes before looking at extension_dir.
var_dump(dl('a/b.so'));
var_dump(dl('../x'));
try { dl([]); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
try { dl(); } catch (\ArgumentCountError $e) { echo $e->getMessage(), "\n"; }
