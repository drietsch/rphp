<?php
// Without declare(ticks=N) a registered tick function never runs.
function tick($x = null) { echo "tick $x\n"; }
var_dump(register_tick_function('tick'));
var_dump(register_tick_function('tick', 'arg'));
var_dump(register_tick_function(fn() => print("closure\n")));
$a = 1;
$a++;
echo "work done\n";
var_dump(unregister_tick_function('tick'));
var_dump(unregister_tick_function('strlen'));
foreach (['nope', ['A', 'b'], [1, 2], 5] as $cb) {
    try { register_tick_function($cb); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
    try { unregister_tick_function($cb); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
}
try { register_tick_function(); } catch (\ArgumentCountError $e) { echo $e->getMessage(), "\n"; }
