<?php
// stream_wrapper_register / unregister / restore and the table they change.
class W {}
var_dump(stream_wrapper_register('ab', 'W'));
var_dump(stream_wrapper_register('ab', 'W'));
var_dump(stream_wrapper_register('file', 'W'));
var_dump(stream_wrapper_register('a b', 'W'));
var_dump(stream_wrapper_register('a_b', 'W'));
var_dump(stream_wrapper_register('a+b-c.d', 'W'));
var_dump(stream_register_wrapper('cd', 'W', STREAM_IS_URL));
try { stream_wrapper_register('xy', 'NoSuchClass'); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
try { stream_wrapper_register([], 'W'); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
$w = stream_get_wrappers();
var_dump(in_array('ab', $w), in_array('cd', $w), in_array('a+b-c.d', $w));
var_dump(array_slice($w, -3));
var_dump(stream_wrapper_unregister('ab'));
var_dump(stream_wrapper_unregister('ab'));
var_dump(stream_wrapper_unregister('zz'));
var_dump(in_array('ab', stream_get_wrappers()));
var_dump(stream_wrapper_register('ab', 'W'));

echo "-- built-ins\n";
var_dump(stream_wrapper_restore('file'));
var_dump(stream_wrapper_restore('zz'));
var_dump(stream_wrapper_restore('ab'));
var_dump(stream_wrapper_unregister('php'));
var_dump(fopen('php://memory', 'r'));
var_dump(in_array('php', stream_get_wrappers()));
var_dump(stream_wrapper_restore('php'));
var_dump(end(stream_get_wrappers()) === 'php' ? 'last' : 'elsewhere');
var_dump(get_resource_type(fopen('php://memory', 'r')));

echo "-- unknown schemes\n";
var_dump(file_exists('nosuch://x'));
var_dump(@fopen('nosuch://x', 'r'));
var_dump(file_get_contents('nosuch://x'));
