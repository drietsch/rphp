<?php
header('X-Custom: yes'); header('Content-Type: text/plain'); http_response_code(201); setcookie('a', 'b', ['path' => '/', 'httponly' => true]); header('Location: /elsewhere', true);
echo "body\n";
