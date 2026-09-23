<?php
// get_resources() (resource 4 is the default stream context), fprintf() and vfprintf().
$a = fopen('php://memory', 'r+'); $b = opendir('.');
var_dump(get_resources()); var_dump(array_keys(get_resources('stream'))); var_dump(stream_context_get_default());
try { get_resources('nope'); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
var_dump(fprintf($a, "%05d|%s", 42, "x"), vfprintf($a, "[%s-%s]", ["a", "b"])); rewind($a); var_dump(stream_get_contents($a));
var_dump(fprintf(STDOUT, "%s\n", "to stdout"));
try { fprintf("x", "a"); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { fprintf($a, "%d %d", 1); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { vfprintf($a, "%d", "x"); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { vfprintf($a, "%d %d", [1]); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$r = fopen('php://memory', 'r'); var_dump(fprintf($r, "abc"));
