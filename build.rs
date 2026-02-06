fn main() {
    // Protobuf 编译（FutuOpenD proto 文件）
    // 当前使用 JSON 模式与 FutuOpenD 通信，proto 编译预留
    // 如需启用 protobuf 模式，取消以下注释并添加 .proto 文件到 proto/ 目录

    // prost_build::compile_protos(&["proto/Qot_Common.proto"], &["proto/"]).unwrap();

    // macOS 框架链接
    println!("cargo:rustc-link-lib=framework=ApplicationServices");
}
