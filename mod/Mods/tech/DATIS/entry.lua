declare_plugin("DATIS", {
  installed     = true,
  dirName       = current_mod_path,
  binaries      =  {
      "datis.dll",
  },

  version       = "0.3.0",
  state         = "installed",
  developerName = "github.com/rkusa",

  Options = {
    {
      name   = "DCS ATIS",
      nameId = "DATIS",
      dir    = "Options",
    },
  },
})

plugin_done()
