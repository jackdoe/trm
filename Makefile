APP := Trm.app

all: $(APP)

target/release/trm: Cargo.toml $(wildcard src/*.rs)
	cargo build --release

target/trm.icns:
	mkdir -p target/trm.iconset
	osascript -l JavaScript -e 'ObjC.import("Cocoa"); var i=$$.NSImage.alloc.initWithSize($$.NSMakeSize(1024,1024)); i.lockFocus; $$.NSColor.blackColor.setFill; $$.NSBezierPath.fillRect($$.NSMakeRect(0,0,1024,1024)); i.unlockFocus; var r=$$.NSBitmapImageRep.imageRepWithData(i.TIFFRepresentation); r.representationUsingTypeProperties(4,$$()).writeToFileAtomically("target/icon.png", true)'
	for s in 16 32 128 256 512; do \
		sips -z $$s $$s target/icon.png --out target/trm.iconset/icon_$${s}x$${s}.png >/dev/null; \
		sips -z $$((s*2)) $$((s*2)) target/icon.png --out target/trm.iconset/icon_$${s}x$${s}@2x.png >/dev/null; \
	done
	iconutil -c icns target/trm.iconset -o $@

$(APP): target/release/trm target/trm.icns
	mkdir -p $(APP)/Contents/MacOS $(APP)/Contents/Resources
	cp target/release/trm $(APP)/Contents/MacOS/trm
	cp target/trm.icns $(APP)/Contents/Resources/trm.icns
	printf '%s\n' \
		'<?xml version="1.0" encoding="UTF-8"?>' \
		'<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
		'<plist version="1.0"><dict>' \
		'<key>CFBundleExecutable</key><string>trm</string>' \
		'<key>CFBundleIconFile</key><string>trm</string>' \
		'<key>CFBundleIdentifier</key><string>dev.trm</string>' \
		'<key>CFBundleName</key><string>Trm</string>' \
		'<key>CFBundlePackageType</key><string>APPL</string>' \
		'<key>NSHighResolutionCapable</key><true/>' \
		'</dict></plist>' > $(APP)/Contents/Info.plist
	codesign --force -s - -o runtime $(APP)
	touch $(APP)

run: $(APP)
	open $(APP)

test:
	cargo test --release

clean:
	cargo clean
	rm -rf $(APP)

.PHONY: all run test clean
