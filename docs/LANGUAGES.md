# 🌐 Akiro Supported Languages & Optimization Matrix

Akiro natively supports **18 programming languages and runtime engines**, pre-configured with competitive programming optimizations (PCH, AtCoder Library, JVM CDS, JIT pre-warming, and persistent compiler caches).

---

## 📊 Summary Table

| Language | Identifiers / Aliases | Compiler / Engine | Version | Compilation & Execution Flags |
| :--- | :--- | :--- | :--- | :--- |
| **C++** | `cpp`, `c++` | G++ | 12.2+ (C++20) | `g++ -O3 -std=c++20 -include <bits/stdc++.h> -I/usr/local/include` (with AtCoder Library ACL) |
| **C** | `c` | GCC | 12.2+ | `gcc -O3 -march=native` |
| **Python** | `python`, `py`, `python3` | CPython | 3.11+ | `python3 -B -O` with precompiled standard library `.pyc` |
| **PyPy** | `pypy`, `pypy3` | PyPy3 | 7.3+ | `pypy3` with warm JIT compiler |
| **Java** | `java` | OpenJDK | 17 LTS | `javac -J-Xms16m -J-Xmx128m` + `java -Xshare:on -XX:+TieredStopAtLevel=1 -Xms16m -Xmx128m` |
| **JavaScript** | `javascript`, `js` | Bun | 1.1+ | `bun run` high-performance V8 runtime |
| **TypeScript** | `typescript`, `ts` | Bun TS | 1.1+ | `bun run` native zero-transpile TypeScript JIT |
| **Rust** | `rust`, `rs` | Rustc | 1.75+ | `rustc -O -C opt-level=3 -C codegen-units=1` |
| **Go** | `go`, `golang` | Golang | 1.22+ | `go build` with persistent `/var/cache/gocache` |
| **Kotlin** | `kotlin`, `kt` | Kotlinc + JVM | 2.1.10 | `kotlinc -include-runtime` + single-tier JIT JVM runner |
| **C#** | `csharp`, `cs` | Mono MCS | 6.8+ | `mcs -optimize+` + `mono --optimize=all` |
| **Zig** | `zig` | Zig | 0.13.0 | `zig build-exe -O ReleaseFast` |
| **Ruby** | `ruby`, `rb` | Ruby | 3.1+ | `ruby --disable-gems` |
| **PHP** | `php` | PHP CLI | 8.2+ | `php -d opcache.enable_cli=1 -d opcache.jit=tracing` |
| **Haskell** | `haskell`, `hs` | GHC | 9.0.2 | `ghc -O2 -v0` native compilation |
| **Dart** | `dart` | Dart SDK | 3.0+ | `dart compile exe` AOT machine code binary |
| **Scala** | `scala` | Scalac + OpenJDK | 2.11+ / 17 | `scalac -d /sandbox` + direct `java -cp` JVM execution |
| **SQL** | `sql`, `sqlite` | SQLite3 | 3.40+ | `sqlite3 :memory:` with CSV/table parsing |

---

## 💻 Sample Code for All 18 Languages

### 1. C++ (`cpp`)
```cpp
#include <iostream>
using namespace std;

int main() {
    ios_base::sync_with_stdio(false);
    cin.tie(NULL);
    long long a, b;
    if (cin >> a >> b) {
        cout << a + b << "\n";
    }
    return 0;
}
```

### 2. C (`c`)
```c
#include <stdio.h>

int main() {
    long long a, b;
    if (scanf("%lld %lld", &a, &b) == 2) {
        printf("%lld\n", a + b);
    }
    return 0;
}
```

### 3. Python 3 (`python`) / PyPy 3 (`pypy`)
```python
import sys

def main():
    data = sys.stdin.read().split()
    if data:
        print(int(data[0]) + int(data[1]))

if __name__ == "__main__":
    main()
```

### 4. Java (`java`)
```java
import java.util.Scanner;

public class Solution {
    public static void main(String[] args) {
        Scanner sc = new Scanner(System.in);
        if (sc.hasNextLong()) {
            long a = sc.nextLong();
            long b = sc.nextLong();
            System.out.println(a + b);
        }
    }
}
```

### 5. JavaScript (`javascript`) / TypeScript (`typescript`)
```typescript
import * as fs from "fs";

const input = fs.readFileSync(0, "utf-8").trim().split(/\s+/);
if (input.length >= 2) {
    const a = parseInt(input[0], 10);
    const b = parseInt(input[1], 10);
    console.log(a + b);
}
```

### 6. Rust (`rust`)
```rust
use std::io::{self, Read};

fn main() {
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer).unwrap();
    let mut iter = buffer.split_whitespace();
    if let (Some(a), Some(b)) = (iter.next(), iter.next()) {
        let a: i64 = a.parse().unwrap();
        let b: i64 = b.parse().unwrap();
        println!("{}", a + b);
    }
}
```

### 7. Go (`go`)
```go
package main

import (
    "fmt"
)

func main() {
    var a, b int64
    if _, err := fmt.Scan(&a, &b); err == nil {
        fmt.Println(a + b)
    }
}
```

### 8. Kotlin (`kotlin`)
```kotlin
import java.util.Scanner

fun main() {
    val scanner = Scanner(System.`in`)
    if (scanner.hasNextLong()) {
        val a = scanner.nextLong()
        val b = scanner.nextLong()
        println(a + b)
    }
}
```

### 9. C# (.NET / Mono) (`csharp`)
```csharp
using System;

class Solution {
    static void Main() {
        string line = Console.ReadLine();
        if (line != null) {
            string[] parts = line.Trim().Split();
            long a = long.Parse(parts[0]);
            long b = long.Parse(parts[1]);
            Console.WriteLine(a + b);
        }
    }
}
```

### 10. Zig (`zig`)
```zig
const std = @import("std");

pub fn main() !void {
    var stdin = std.io.getStdIn().reader();
    var buf: [128]u8 = undefined;
    if (try stdin.readUntilDelimiterOrEof(&buf, '\n')) |line| {
        var it = std.mem.tokenizeScalar(u8, line, ' ');
        const a_str = it.next() orelse return;
        const b_str = it.next() orelse return;
        const a = try std.fmt.parseInt(i64, a_str, 10);
        const b = try std.fmt.parseInt(i64, b_str, 10);
        const stdout = std.io.getStdOut().writer();
        try stdout.print("{d}\n", .{a + b});
    }
}
```

### 11. Ruby (`ruby`)
```ruby
input = gets
if input
  a, b = input.split.map(&:to_i)
  puts a + b
end
```

### 12. PHP (`php`)
```php
<?php
$line = trim(fgets(STDIN));
if ($line !== "") {
    $parts = explode(" ", $line);
    echo ((int)$parts[0] + (int)$parts[1]) . "\n";
}
?>
```

### 13. Haskell (`haskell`)
```haskell
main :: IO ()
main = do
    input <- getContents
    let nums = map read (words input) :: [Integer]
    case nums of
        (a:b:_) -> print (a + b)
        _       -> return ()
```

### 14. Dart (`dart`)
```dart
import 'dart:io';

void main() {
  String? line = stdin.readLineSync();
  if (line != null) {
    List<String> parts = line.trim().split(' ');
    int a = int.parse(parts[0]);
    int b = int.parse(parts[1]);
    print(a + b);
  }
}
```

### 15. Scala (`scala`)
```scala
import java.util.Scanner

object Solution {
  def main(args: Array[String]): Unit = {
    val sc = new Scanner(System.in)
    if (sc.hasNextLong()) {
      val a = sc.nextLong()
      val b = sc.nextLong()
      println(a + b)
    }
  }
}
```

### 16. SQL (`sql`)
```sql
SELECT 10 + 20 AS result;
```
