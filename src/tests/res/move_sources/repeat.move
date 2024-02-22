module repeat::adder {
    fun sum(n: u32): u32 {
        let i: u32 = 1;
        let total: u32 = 0;
        while (i <= n) {
            total = total + i;
            i = i + 1;
        };
        total
    }

    fun fib(n: u32): u32 {
        let a: u32 = 0;
        let b: u32 = 1;

        if (n == 0) {
            return a;
        } else if (n == 1) {
            return b;
        };

        loop {
            let c = a + b;
            a = b;
            b = c;
            if (n == 1) break;
            n = n - 1;
        };

        b
    }

    fun collatz(n: u32): u32 {
        let count: u32 = 0;
        while (n != 1) {
            if (n % 2 == 0) {
                n = n / 2;
            } else {
                n = 3 * n + 1;
            };
            count = count + 1;
        };
        count
    }

    public entry fun main() {
        assert!(sum(5) == 15, 1);
        assert!(fib(8) == 21, 2);
        assert!(collatz(12) == 9, 3);
    }
}
