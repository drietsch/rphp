<?php var_export(['u' => $_SERVER['PHP_AUTH_USER'] ?? null, 'p' => $_SERVER['PHP_AUTH_PW'] ?? null, 'd' => $_SERVER['PHP_AUTH_DIGEST'] ?? null, 'h' => $_SERVER['HTTP_AUTHORIZATION'] ?? null]);
