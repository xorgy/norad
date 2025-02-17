use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

#[cfg(feature = "rayon")]
use rayon::prelude::*;

use crate::glyph::GlyphName;
use crate::names::NameList;
use crate::shared_types::Color;
use crate::{Error, Glyph, Plist};

static CONTENTS_FILE: &str = "contents.plist";
static LAYER_INFO_FILE: &str = "layerinfo.plist";

/// A [layer], corresponding to a 'glyphs' directory. Conceptually, a layer
/// is just a collection of glyphs.
///
/// [layer]: http://unifiedfontobject.org/versions/ufo3/glyphs/
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    pub(crate) glyphs: BTreeMap<GlyphName, Arc<Glyph>>,
    contents: BTreeMap<GlyphName, PathBuf>,
    pub color: Option<Color>,
    pub lib: Plist,
}

impl Layer {
    /// Load the layer at this path.
    ///
    /// Internal callers should use `load_impl` directly, so that glyph names
    /// can be reused between layers.
    pub fn load(path: impl AsRef<Path>) -> Result<Layer, Error> {
        let path = path.as_ref();
        let names = NameList::default();
        Layer::load_impl(path, &names)
    }

    /// the actual loading logic.
    ///
    /// `names` is a map of glyphnames; we pass it throughout parsing
    /// so that we reuse the same Arc<str> for identical names.
    pub(crate) fn load_impl(path: &Path, names: &NameList) -> Result<Layer, Error> {
        let contents_path = path.join(CONTENTS_FILE);
        // these keys are never used; a future optimization would be to skip the
        // names and deserialize to a vec; that would not be a one-liner, though.
        let contents: BTreeMap<GlyphName, PathBuf> = plist::from_file(contents_path)?;

        #[cfg(feature = "rayon")]
        let iter = contents.par_iter();
        #[cfg(not(feature = "rayon"))]
        let iter = contents.iter();

        let glyphs = iter
            .map(|(name, glyph_path)| {
                let name = names.get(name);
                let glyph_path = path.join(glyph_path);

                Glyph::load_with_names(&glyph_path, names).map(|mut glyph| {
                    glyph.name = name.clone();
                    (name, Arc::new(glyph))
                })
            })
            //FIXME: come up with a better way of reporting errors than just aborting at first failure
            .collect::<Result<_, _>>()?;

        let layerinfo_path = path.join(LAYER_INFO_FILE);
        let (color, lib) = if layerinfo_path.exists() {
            Self::layerinfo_from_file(&layerinfo_path)?
        } else {
            (None, Plist::new())
        };

        Ok(Layer { contents, glyphs, color, lib })
    }

    // Problem: layerinfo.plist contains a nested plist dictionary and the plist crate
    // cannot adequately handle that, as ser/de is not implemented for plist::Value.
    // Ser/de must be done manually...
    fn layerinfo_from_file(path: &Path) -> Result<(Option<Color>, Plist), Error> {
        let mut info_content = plist::Value::from_file(path)
            .map_err(Error::PlistError)?
            .into_dictionary()
            .ok_or(Error::ExpectedPlistDictionaryError)?;

        let mut color = None;
        let color_str = info_content.remove("color");
        if let Some(v) = color_str {
            match v.into_string() {
                Some(s) => color.replace(Color::from_str(&s).map_err(Error::InvalidDataError)?),
                None => return Err(Error::ExpectedPlistStringError),
            };
        };

        let lib = match info_content.remove("lib") {
            Some(v) => v.into_dictionary().ok_or(Error::ExpectedPlistDictionaryError)?,
            None => Plist::new(),
        };

        Ok((color, lib))
    }

    fn layerinfo_to_file(&self, path: &Path) -> Result<(), Error> {
        let mut dict = plist::dictionary::Dictionary::new();

        if let Some(c) = &self.color {
            dict.insert("color".into(), plist::Value::String(c.to_rgba_string()));
        }
        if !self.lib.is_empty() {
            dict.insert("lib".into(), plist::Value::Dictionary(self.lib.clone()));
        }

        plist::Value::Dictionary(dict).to_file_xml(path.join(LAYER_INFO_FILE))?;

        Ok(())
    }

    fn layerinfo_is_empty(&self) -> bool {
        self.color.is_none() && self.lib.is_empty()
    }

    /// Attempt to write this layer to the given path.
    ///
    /// The path should not exist.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        fs::create_dir(&path)?;
        plist::to_file_xml(path.join(CONTENTS_FILE), &self.contents)?;
        // Avoid writing empty layerinfo.plist file.
        if !self.layerinfo_is_empty() {
            self.layerinfo_to_file(&path)?;
        }
        for (name, glyph_path) in self.contents.iter() {
            let glyph = self.glyphs.get(name).expect("all glyphs in contents must exist.");
            glyph.save(path.join(glyph_path))?;
        }

        Ok(())
    }

    /// Returns a reference the glyph with the given name, if it exists.
    pub fn get_glyph<K>(&self, glyph: &K) -> Option<&Arc<Glyph>>
    where
        GlyphName: Borrow<K>,
        K: Ord + ?Sized,
    {
        self.glyphs.get(glyph)
    }

    /// Returns a mutable reference to the glyph with the given name, if it exists.
    pub fn get_glyph_mut<K>(&mut self, glyph: &K) -> Option<&mut Glyph>
    where
        GlyphName: Borrow<K>,
        K: Ord + ?Sized,
    {
        self.glyphs.get_mut(glyph).and_then(|g| Arc::get_mut(g))
    }

    /// Returns `true` if this layer contains a glyph with this name.
    pub fn contains_glyph(&self, name: &str) -> bool {
        self.glyphs.contains_key(name)
    }

    /// Adds or updates the given glyph.
    ///
    /// If the glyph does not previously exist, the filename is calculated from
    /// the glyph's name.
    pub fn insert_glyph(&mut self, glyph: impl Into<Arc<Glyph>>) {
        let glyph = glyph.into();
        if !self.contents.contains_key(&glyph.name) {
            let path = crate::glyph::default_file_name_for_glyph_name(&glyph.name);
            self.contents.insert(glyph.name.clone(), path.into());
        }
        self.glyphs.insert(glyph.name.clone(), glyph);
    }

    /// Remove the named glyph from this layer.
    #[doc(hidden)]
    #[deprecated(since = "0.3.0", note = "use remove_glyph instead")]
    pub fn delete_glyph(&mut self, name: &str) {
        self.remove_glyph(name);
    }

    /// Remove the named glyph from this layer and return it, if it exists.
    pub fn remove_glyph(&mut self, name: &str) -> Option<Arc<Glyph>> {
        self.contents.remove(name);
        self.glyphs.remove(name)
    }

    /// Iterate over the glyphs in this layer.
    pub fn iter_contents(&self) -> impl Iterator<Item = Arc<Glyph>> + '_ {
        self.glyphs.values().map(Arc::clone)
    }

    /// Iterate over the glyphs in this layer, mutably.
    pub fn iter_contents_mut(&mut self) -> impl Iterator<Item = &mut Glyph> {
        self.glyphs.values_mut().map(Arc::make_mut)
    }

    #[cfg(test)]
    pub fn get_path(&self, name: &str) -> Option<&Path> {
        self.contents.get(name).map(PathBuf::as_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn load_layer() {
        let layer_path = "testdata/mutatorSans/MutatorSansBoldWide.ufo/glyphs";
        assert!(Path::new(layer_path).exists(), "missing test data. Did you `git submodule init`?");
        let layer = Layer::load(layer_path).unwrap();
        assert_eq!(
            layer.color.as_ref().unwrap(),
            &Color { red: 1.0, green: 0.75, blue: 0.0, alpha: 0.7 }
        );
        assert_eq!(
            layer.lib.get("com.typemytype.robofont.segmentType").unwrap().as_string().unwrap(),
            "curve"
        );
        let glyph = layer.get_glyph("A").expect("failed to load glyph 'A'");
        assert_eq!(glyph.height, 0.);
        assert_eq!(glyph.width, 1290.);
        assert_eq!(glyph.codepoints, vec!['A']);
    }

    #[test]
    fn load_write_layerinfo() {
        let layer_path = "testdata/mutatorSans/MutatorSansBoldWide.ufo/glyphs";
        assert!(Path::new(layer_path).exists(), "missing test data. Did you `git submodule init`?");
        let mut layer = Layer::load(layer_path).unwrap();

        layer.color.replace(Color { red: 0.5, green: 0.5, blue: 0.5, alpha: 0.5 });
        layer.lib.insert(
            "com.typemytype.robofont.segmentType".into(),
            plist::Value::String("test".into()),
        );

        let temp_dir = tempdir::TempDir::new("test.ufo").unwrap();
        let dir = temp_dir.path().join("glyphs");
        layer.save(&dir).unwrap();
        let layer2 = Layer::load(&dir).unwrap();

        assert_eq!(
            layer2.color.as_ref().unwrap(),
            &Color { red: 0.5, green: 0.5, blue: 0.5, alpha: 0.5 }
        );
        assert_eq!(
            layer2.lib.get("com.typemytype.robofont.segmentType").unwrap().as_string().unwrap(),
            "test"
        );
    }

    #[test]
    fn skip_writing_empty_layerinfo() {
        let mut layer = Layer::default();
        let temp_dir = tempdir::TempDir::new("test.ufo").unwrap();
        let dir = temp_dir.path().join("glyphs");

        layer.save(&dir).unwrap();
        assert!(!dir.join("layerinfo.plist").exists());

        fs::remove_dir_all(&dir).unwrap();
        layer.lib = Plist::new();
        layer.save(&dir).unwrap();
        assert!(!dir.join("layerinfo.plist").exists());
    }

    #[test]
    fn delete() {
        let layer_path = "testdata/mutatorSans/MutatorSansBoldWide.ufo/glyphs";
        let mut layer = Layer::load(layer_path).unwrap();
        layer.remove_glyph("A");
        if let Some(glyph) = layer.get_glyph("A") {
            panic!("{:?}", glyph);
        }

        if let Some(path) = layer.get_path("A") {
            panic!("{:?}", path);
        }
    }

    #[test]
    fn set_glyph() {
        let layer_path = "testdata/mutatorSans/MutatorSansBoldWide.ufo/glyphs";
        let mut layer = Layer::load(layer_path).unwrap();
        let mut glyph = Glyph::new_named("A");
        glyph.width = 69.;
        layer.insert_glyph(glyph);
        let glyph = layer.get_glyph("A").expect("failed to load glyph 'A'");
        assert_eq!(glyph.width, 69.);
    }
}
