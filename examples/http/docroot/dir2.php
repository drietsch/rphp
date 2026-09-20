<?php
$env = array_keys(getenv());
$srv = array_filter($_SERVER, fn($k) => !in_array($k, $env, true), ARRAY_FILTER_USE_KEY);
foreach (['REQUEST_TIME_FLOAT', 'REQUEST_TIME', 'REMOTE_PORT'] as $k) unset($srv[$k]);
echo "index.php\n";
var_export($srv); echo "\n";
var_export(['GET' => $_GET, 'POST' => $_POST, 'COOKIE' => $_COOKIE, 'FILES' => $_FILES, 'REQUEST' => $_REQUEST]); echo "\n";
var_export(['input' => file_get_contents('php://input'), 'headers' => getallheaders(), 'sapi' => PHP_SAPI, 'cwd' => getcwd() === __DIR__]); echo "\n";
