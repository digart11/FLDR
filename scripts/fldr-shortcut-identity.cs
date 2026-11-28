// Only the application's identity property changes; other Shell-link fields stay intact.
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
public static class FldrShortcutIdentity {
    [StructLayout(LayoutKind.Sequential)] struct Key { public Guid Format; public uint Id; }
    [StructLayout(LayoutKind.Explicit, Size = 24)] struct Variant {
        [FieldOffset(0)] public ushort Type;
        [FieldOffset(8)] public IntPtr Text;
    }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface Store {
        [PreserveSig] int GetCount(out uint count);
        [PreserveSig] int GetAt(uint index, out Key key);
        [PreserveSig] int GetValue(ref Key key, out Variant value);
        [PreserveSig] int SetValue(ref Key key, ref Variant value);
        [PreserveSig] int Commit();
    }
    [DllImport("ole32.dll")] static extern int PropVariantClear(ref Variant value);
    static Key AppId() { return new Key { Format = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), Id = 5 }; }
    static object Open(string path) {
        object link = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("00021401-0000-0000-C000-000000000046")));
        try { ((IPersistFile)link).Load(path, 2); return link; }
        catch { Marshal.FinalReleaseComObject(link); throw; }
    }
    public static void Set(string path) {
        object link = Open(path);
        Variant value = new Variant { Type = 31, Text = Marshal.StringToCoTaskMemUni("digART.FLDR") };
        try {
            Key key = AppId(); Store store = (Store)link;
            Marshal.ThrowExceptionForHR(store.SetValue(ref key, ref value));
            Marshal.ThrowExceptionForHR(store.Commit());
            ((IPersistFile)link).Save(path, true);
        } finally { PropVariantClear(ref value); Marshal.FinalReleaseComObject(link); }
    }
    public static string Read(string path) {
        object link = Open(path); Variant value = new Variant();
        try {
            Key key = AppId(); Marshal.ThrowExceptionForHR(((Store)link).GetValue(ref key, out value));
            return value.Type == 31 ? Marshal.PtrToStringUni(value.Text) : null;
        } finally { PropVariantClear(ref value); Marshal.FinalReleaseComObject(link); }
    }
}
