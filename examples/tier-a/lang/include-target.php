<?php
// The file lang/include.php includes; harmless when run on its own. It
// declares a function and a class, sets variables in the includer's scope,
// reads one from it, and returns a value.
if (!function_exists('from_target')) {
    function from_target() { return 'target-fn'; }
}
if (!class_exists('FromTarget')) {
    class FromTarget { public $origin = 'target'; }
}
$set_by_target = isset($set_by_includer) ? "saw:$set_by_includer" : 'standalone';
echo basename(__FILE__), ' in ', basename(__DIR__), ' line ', __LINE__, "\n";
return ['returned' => true, 'count' => ($include_count ?? 0) + 1];
