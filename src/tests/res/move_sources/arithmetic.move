module arithmetic::add {
    fun add(x: u32, y: u32): u32 {
        x + y
    }

    fun sub(x: u32, y: u32): u32 {
        x - y
    }

    fun mul(x: u32, y: u32): u32 {
        x * y
    }

    fun div(x: u32, y: u32): u32 {
        x / y
    }

    fun mod(x: u32, y: u32): u32 {
        x % y
    }

    public entry fun main() {
        assert!(add(2, 3) == 5, 1);
        assert!(sub(7, 4) == 3, 2);
        assert!(mul(7, 4) == 28, 3);
        assert!(div(7, 4) == 1, 4);
        assert!(mod(7, 4) == 3, 5);
    }
}
