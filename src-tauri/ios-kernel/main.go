// iOS 内嵌内核：sing-box 编译为 c-archive 静态库，Rust 经 FFI 调用。
//
// 与桌面 sidecar 的差异仅在于承载方式（进程内 vs 子进程）：
// 配置 JSON / clash_api / geo 数据库逻辑完全一致。
//
// 构建（CI macOS）：
//   CGO_ENABLED=1 GOOS=ios GOARCH=arm64 CC=$(xcrun -sdk iphoneos -f clang) \
//     go build -buildmode=c-archive -tags "with_gvisor,with_quic,with_grpc,with_wireguard,with_ech,with_utls,with_clash_api" \
//     -trimpath -ldflags="-s -w" -o libprismkernel.a .
package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"time"
	"unsafe"

	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/experimental/deprecated"
	"github.com/sagernet/sing-box/include"
	"github.com/sagernet/sing-box/log"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json"
	"github.com/sagernet/sing/service"
)

var (
	mu       sync.Mutex
	instance *box.Box
	cancel   context.CancelFunc
)

//export PrismKernelStart
func PrismKernelStart(configPath *C.char, workDir *C.char) *C.char {
	mu.Lock()
	defer mu.Unlock()
	if instance != nil {
		return nil // 已在运行（Rust 侧状态机保证不重复启动，双保险）
	}

	// 与 desktop spawn 一致：Clash 订阅转换仍产出 legacy geoip/geosite 字段
	os.Setenv("ENABLE_DEPRECATED_GEOIP", "true")
	os.Setenv("ENABLE_DEPRECATED_GEOSITE", "true")

	wd := C.GoString(workDir)
	if wd != "" {
		_ = os.MkdirAll(wd, 0o755)
		if err := os.Chdir(wd); err != nil {
			return C.CString("chdir workdir: " + err.Error())
		}
	}

	// 内核日志写 workDir/kernel.log（iOS 无父进程接管 stderr，与桌面日志位置对齐）
	if wd != "" {
		if f, err := os.Create(filepath.Join(wd, "kernel.log")); err == nil {
			log.SetStdLogger(log.NewDefaultFactory(
				context.Background(),
				log.Formatter{BaseTime: time.Now(), DisableColors: true},
				f, "", nil, false,
			).Logger())
		}
	}

	content, err := os.ReadFile(C.GoString(configPath))
	if err != nil {
		return C.CString("read config: " + err.Error())
	}

	ctx := service.ContextWith(context.Background(), deprecated.NewStderrManager(log.StdLogger()))
	ctx = box.Context(ctx, include.InboundRegistry(), include.OutboundRegistry(), include.EndpointRegistry())

	options, err := json.UnmarshalExtendedContext[option.Options](ctx, content)
	if err != nil {
		return C.CString("decode config: " + err.Error())
	}
	if options.Log == nil {
		options.Log = &option.LogOptions{}
	}
	options.Log.DisableColor = true

	bctx, cancelFn := context.WithCancel(ctx)
	boxInstance, err := box.New(box.Options{Context: bctx, Options: options})
	if err != nil {
		cancelFn()
		return C.CString("create: " + err.Error())
	}
	if err := boxInstance.Start(); err != nil {
		cancelFn()
		return C.CString("start: " + err.Error())
	}
	instance = boxInstance
	cancel = cancelFn
	return nil
}

//export PrismKernelCheck
func PrismKernelCheck(configPath *C.char, workDir *C.char) *C.char {
	// 与 sing-box check 命令等价：解析配置 + box.New 构建后立即关闭。
	// 重启前调用——校验失败时旧内核保留，网络不中断。
	os.Setenv("ENABLE_DEPRECATED_GEOIP", "true")
	os.Setenv("ENABLE_DEPRECATED_GEOSITE", "true")

	wd := C.GoString(workDir)
	if wd != "" {
		_ = os.MkdirAll(wd, 0o755)
		if err := os.Chdir(wd); err != nil {
			return C.CString("chdir workdir: " + err.Error())
		}
	}

	content, err := os.ReadFile(C.GoString(configPath))
	if err != nil {
		return C.CString("read config: " + err.Error())
	}

	ctx := service.ContextWith(context.Background(), deprecated.NewStderrManager(log.StdLogger()))
	ctx = box.Context(ctx, include.InboundRegistry(), include.OutboundRegistry(), include.EndpointRegistry())

	options, err := json.UnmarshalExtendedContext[option.Options](ctx, content)
	if err != nil {
		return C.CString("decode config: " + err.Error())
	}

	bctx, cancelFn := context.WithCancel(ctx)
	boxInstance, err := box.New(box.Options{Context: bctx, Options: options})
	cancelFn()
	if err != nil {
		return C.CString("create: " + err.Error())
	}
	_ = boxInstance.Close()
	return nil
}

//export PrismKernelStop
func PrismKernelStop() {
	mu.Lock()
	defer mu.Unlock()
	if instance != nil {
		_ = instance.Close()
		cancel()
		instance = nil
		cancel = nil
	}
}

//export PrismKernelFree
func PrismKernelFree(p *C.char) {
	C.free(unsafe.Pointer(p))
}

func main() {}
