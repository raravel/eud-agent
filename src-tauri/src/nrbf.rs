use std::collections::BTreeMap;

pub const SERIALIZED_STREAM_HEADER: u8 = 0;
pub const BINARY_OBJECT_STRING: u8 = 6;
pub const MESSAGE_END: u8 = 11;

const MAX_STRING_BYTES: usize = 32 * 1024 * 1024;
const MAX_COLLECTION_ITEMS: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Primitive {
    pub kind: u8,
    pub data: Vec<u8>,
}

impl Primitive {
    pub fn boolean(value: bool) -> Self {
        Self {
            kind: 1,
            data: vec![u8::from(value)],
        }
    }

    pub fn int32(value: i32) -> Self {
        Self {
            kind: 8,
            data: value.to_le_bytes().to_vec(),
        }
    }

    pub fn date_time(value: u64) -> Self {
        Self {
            kind: 13,
            data: value.to_le_bytes().to_vec(),
        }
    }

    pub fn as_i64(&self) -> Result<i64, String> {
        match self.kind {
            1 | 2 => Ok(i64::from(self.data[0])),
            7 => Ok(i64::from(i16::from_le_bytes(self.array()?))),
            8 => Ok(i64::from(i32::from_le_bytes(self.array()?))),
            9 => Ok(i64::from_le_bytes(self.array()?)),
            10 => Ok(i64::from(i8::from_le_bytes(self.array()?))),
            14 => Ok(i64::from(u16::from_le_bytes(self.array()?))),
            15 => Ok(i64::from(u32::from_le_bytes(self.array()?))),
            16 => i64::try_from(u64::from_le_bytes(self.array()?))
                .map_err(|_| "NRBF UInt64 exceeds i64".to_string()),
            other => Err(format!("NRBF primitive type {other} is not an integer")),
        }
    }

    pub fn as_bool(&self) -> Result<bool, String> {
        if self.kind != 1 || self.data.len() != 1 {
            return Err("NRBF primitive is not Boolean".to_string());
        }
        Ok(self.data[0] != 0)
    }

    pub fn set_i64(&mut self, value: i64) -> Result<(), String> {
        self.data = match self.kind {
            1 => vec![u8::from(value != 0)],
            2 => vec![u8::try_from(value).map_err(|_| "Byte overflow".to_string())?],
            7 => i16::try_from(value)
                .map_err(|_| "Int16 overflow".to_string())?
                .to_le_bytes()
                .to_vec(),
            8 => i32::try_from(value)
                .map_err(|_| "Int32 overflow".to_string())?
                .to_le_bytes()
                .to_vec(),
            9 => value.to_le_bytes().to_vec(),
            10 => i8::try_from(value)
                .map_err(|_| "SByte overflow".to_string())?
                .to_le_bytes()
                .to_vec(),
            14 => u16::try_from(value)
                .map_err(|_| "UInt16 overflow".to_string())?
                .to_le_bytes()
                .to_vec(),
            15 => u32::try_from(value)
                .map_err(|_| "UInt32 overflow".to_string())?
                .to_le_bytes()
                .to_vec(),
            16 => u64::try_from(value)
                .map_err(|_| "UInt64 overflow".to_string())?
                .to_le_bytes()
                .to_vec(),
            other => return Err(format!("NRBF primitive type {other} is not an integer")),
        };
        Ok(())
    }

    fn array<const N: usize>(&self) -> Result<[u8; N], String> {
        self.data
            .as_slice()
            .try_into()
            .map_err(|_| format!("NRBF primitive expected {N} bytes"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdditionalTypeInfo {
    Primitive(u8),
    SystemClass(String),
    Class { name: String, library_id: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberType {
    pub binary_type: u8,
    pub additional: Option<AdditionalTypeInfo>,
}

impl MemberType {
    pub fn primitive_kind(&self) -> Option<u8> {
        match self.additional {
            Some(AdditionalTypeInfo::Primitive(kind)) if self.binary_type == 0 => Some(kind),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassMetadata {
    pub object_id: i32,
    pub name: String,
    pub members: Vec<String>,
    pub member_types: Vec<MemberType>,
    pub library_id: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Primitive(Primitive),
    Record(Record),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassObject {
    pub record_type: u8,
    pub metadata_id: i32,
    pub metadata: Option<ClassMetadata>,
    pub fields: Vec<FieldValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrayObject {
    Binary {
        array_type: u8,
        lengths: Vec<i32>,
        lower_bounds: Vec<i32>,
        member_type: MemberType,
        values: ArrayValues,
    },
    Primitive {
        primitive_type: u8,
        values: Vec<Primitive>,
    },
    Object {
        string_only: bool,
        length: usize,
        values: Vec<Record>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrayValues {
    Primitive(Vec<Primitive>),
    Records(Vec<Record>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectValue {
    String(String),
    Class(ClassObject),
    Array(ArrayObject),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub id: i32,
    pub value: ObjectValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Header {
        root_id: i32,
        header_id: i32,
        major_version: i32,
        minor_version: i32,
    },
    Object(i32),
    Reference(i32),
    Null,
    NullMultiple {
        count: usize,
        small: bool,
    },
    PrimitiveTyped(Primitive),
    Library {
        id: i32,
        name: String,
    },
    End,
}

impl Record {
    pub fn logical_count(&self) -> usize {
        match self {
            Self::NullMultiple { count, .. } => *count,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub records: Vec<Record>,
    pub objects: BTreeMap<i32, Object>,
    pub metadata: BTreeMap<i32, ClassMetadata>,
    next_object_id: i32,
}

impl Document {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut reader = Reader::new(bytes);
        let mut records = Vec::new();
        while !reader.done() {
            let record = reader.read_record()?;
            let is_end = matches!(record, Record::End);
            records.push(record);
            if is_end {
                break;
            }
        }
        if reader.position != bytes.len() {
            return Err("NRBF has bytes after MessageEnd".to_string());
        }
        if !matches!(records.first(), Some(Record::Header { .. }))
            || !matches!(records.last(), Some(Record::End))
        {
            return Err("NRBF requires SerializedStreamHeader and MessageEnd".to_string());
        }
        let max_magnitude = reader
            .objects
            .keys()
            .map(|id| id.unsigned_abs())
            .max()
            .unwrap_or(0);
        let next_object_id = i32::try_from(max_magnitude.saturating_add(1))
            .map_err(|_| "NRBF object id space is exhausted".to_string())?;
        Ok(Self {
            records,
            objects: reader.objects,
            metadata: reader.metadata,
            next_object_id,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut output = Vec::new();
        for record in &self.records {
            self.write_record(record, &mut output)?;
        }
        Ok(output)
    }

    pub fn root_id(&self) -> Result<i32, String> {
        match self.records.first() {
            Some(Record::Header { root_id, .. }) => Ok(*root_id),
            _ => Err("NRBF header is missing".to_string()),
        }
    }

    pub fn object(&self, id: i32) -> Result<&Object, String> {
        self.objects
            .get(&id)
            .ok_or_else(|| format!("NRBF object {id} is missing"))
    }

    pub fn object_mut(&mut self, id: i32) -> Result<&mut Object, String> {
        self.objects
            .get_mut(&id)
            .ok_or_else(|| format!("NRBF object {id} is missing"))
    }

    pub fn resolved_id(&self, record: &Record) -> Result<Option<i32>, String> {
        match record {
            Record::Object(id) | Record::Reference(id) => {
                self.object(*id)?;
                Ok(Some(*id))
            }
            Record::Null => Ok(None),
            _ => Err("NRBF value is not an object reference".to_string()),
        }
    }

    pub fn class_name(&self, id: i32) -> Result<&str, String> {
        let ObjectValue::Class(class) = &self.object(id)?.value else {
            return Err(format!("NRBF object {id} is not a class"));
        };
        Ok(&self.class_metadata(class)?.name)
    }

    pub fn class_metadata<'a>(
        &'a self,
        class: &'a ClassObject,
    ) -> Result<&'a ClassMetadata, String> {
        match &class.metadata {
            Some(metadata) => Ok(metadata),
            None => self
                .metadata
                .get(&class.metadata_id)
                .ok_or_else(|| format!("NRBF metadata {} is missing", class.metadata_id)),
        }
    }

    pub fn metadata_id(&self, class_name: &str) -> Result<i32, String> {
        self.metadata
            .iter()
            .find_map(|(id, metadata)| (metadata.name == class_name).then_some(*id))
            .ok_or_else(|| format!("NRBF metadata for {class_name} is missing"))
    }

    pub fn field(&self, object_id: i32, name: &str) -> Result<&FieldValue, String> {
        let ObjectValue::Class(class) = &self.object(object_id)?.value else {
            return Err(format!("NRBF object {object_id} is not a class"));
        };
        let index = self
            .class_metadata(class)?
            .members
            .iter()
            .position(|member| member == name)
            .ok_or_else(|| {
                format!(
                    "NRBF class {} has no field {name}",
                    self.class_name(object_id).unwrap_or("?")
                )
            })?;
        class
            .fields
            .get(index)
            .ok_or_else(|| format!("NRBF field {name} is missing"))
    }

    pub fn field_mut(&mut self, object_id: i32, name: &str) -> Result<&mut FieldValue, String> {
        let index = {
            let object = self.object(object_id)?;
            let ObjectValue::Class(class) = &object.value else {
                return Err(format!("NRBF object {object_id} is not a class"));
            };
            self.class_metadata(class)?
                .members
                .iter()
                .position(|member| member == name)
                .ok_or_else(|| format!("NRBF class has no field {name}"))?
        };
        let ObjectValue::Class(class) = &mut self.object_mut(object_id)?.value else {
            unreachable!();
        };
        Ok(&mut class.fields[index])
    }

    pub fn field_object_id(&self, object_id: i32, name: &str) -> Result<Option<i32>, String> {
        let FieldValue::Record(record) = self.field(object_id, name)? else {
            return Err(format!("NRBF field {name} is primitive"));
        };
        self.resolved_id(record)
    }

    pub fn field_i64(&self, object_id: i32, name: &str) -> Result<i64, String> {
        match self.field(object_id, name)? {
            FieldValue::Primitive(value) => value.as_i64(),
            FieldValue::Record(record) => {
                let id = self
                    .resolved_id(record)?
                    .ok_or_else(|| format!("NRBF enum field {name} is null"))?;
                match self.field(id, "value__")? {
                    FieldValue::Primitive(value) => value.as_i64(),
                    _ => Err(format!("NRBF enum field {name} is invalid")),
                }
            }
        }
    }

    pub fn field_bool(&self, object_id: i32, name: &str) -> Result<bool, String> {
        match self.field(object_id, name)? {
            FieldValue::Primitive(value) => value.as_bool(),
            _ => Err(format!("NRBF field {name} is not Boolean")),
        }
    }

    pub fn field_string(&self, object_id: i32, name: &str) -> Result<Option<&str>, String> {
        let Some(id) = self.field_object_id(object_id, name)? else {
            return Ok(None);
        };
        match &self.object(id)?.value {
            ObjectValue::String(value) => Ok(Some(value)),
            _ => Err(format!("NRBF field {name} is not String")),
        }
    }

    pub fn set_field_i64(&mut self, object_id: i32, name: &str, value: i64) -> Result<(), String> {
        match self.field_mut(object_id, name)? {
            FieldValue::Primitive(primitive) => primitive.set_i64(value),
            FieldValue::Record(record) => {
                let target = match record {
                    Record::Object(id) | Record::Reference(id) => *id,
                    _ => return Err(format!("NRBF enum field {name} is null")),
                };
                match self.field_mut(target, "value__")? {
                    FieldValue::Primitive(primitive) => primitive.set_i64(value),
                    _ => Err(format!("NRBF enum field {name} is invalid")),
                }
            }
        }
    }

    pub fn set_field_bool(
        &mut self,
        object_id: i32,
        name: &str,
        value: bool,
    ) -> Result<(), String> {
        match self.field_mut(object_id, name)? {
            FieldValue::Primitive(primitive) if primitive.kind == 1 => {
                *primitive = Primitive::boolean(value);
                Ok(())
            }
            _ => Err(format!("NRBF field {name} is not Boolean")),
        }
    }

    pub fn set_field_record(
        &mut self,
        object_id: i32,
        name: &str,
        record: Record,
    ) -> Result<(), String> {
        match self.field_mut(object_id, name)? {
            FieldValue::Record(value) => {
                *value = record;
                Ok(())
            }
            _ => Err(format!("NRBF field {name} is primitive")),
        }
    }

    pub fn set_field_string(
        &mut self,
        object_id: i32,
        name: &str,
        value: &str,
    ) -> Result<i32, String> {
        let string_id = self.add_string(value.to_string())?;
        self.records
            .retain(|record| !matches!(record, Record::Object(id) if *id == string_id));
        self.set_field_record(object_id, name, Record::Object(string_id))?;
        Ok(string_id)
    }

    pub fn array_records(&self, object_id: i32) -> Result<Vec<Record>, String> {
        let ObjectValue::Array(array) = &self.object(object_id)?.value else {
            return Err(format!("NRBF object {object_id} is not an array"));
        };
        let records = match array {
            ArrayObject::Object { values, .. } => values,
            ArrayObject::Binary {
                values: ArrayValues::Records(values),
                ..
            } => values,
            _ => return Err(format!("NRBF array {object_id} does not contain records")),
        };
        let mut expanded = Vec::new();
        for record in records {
            match record {
                Record::NullMultiple { count, .. } => {
                    expanded.extend(std::iter::repeat(Record::Null).take(*count));
                }
                other => expanded.push(other.clone()),
            }
        }
        Ok(expanded)
    }

    pub fn array_primitives(&self, object_id: i32) -> Result<&[Primitive], String> {
        let ObjectValue::Array(array) = &self.object(object_id)?.value else {
            return Err(format!("NRBF object {object_id} is not an array"));
        };
        match array {
            ArrayObject::Primitive { values, .. }
            | ArrayObject::Binary {
                values: ArrayValues::Primitive(values),
                ..
            } => Ok(values),
            _ => Err(format!("NRBF array {object_id} is not primitive")),
        }
    }
    pub fn set_array_records(&mut self, object_id: i32, values: Vec<Record>) -> Result<(), String> {
        if values.len() > MAX_COLLECTION_ITEMS {
            return Err("NRBF array exceeds item limit".to_string());
        }
        let ObjectValue::Array(array) = &mut self.object_mut(object_id)?.value else {
            return Err(format!("NRBF object {object_id} is not an array"));
        };
        match array {
            ArrayObject::Object {
                length,
                values: target,
                ..
            } => {
                *length = values.len();
                *target = values;
                Ok(())
            }
            ArrayObject::Binary {
                lengths,
                values: ArrayValues::Records(target),
                ..
            } if lengths.len() == 1 => {
                lengths[0] = i32::try_from(values.len())
                    .map_err(|_| "NRBF array is too large".to_string())?;
                *target = values;
                Ok(())
            }
            _ => Err(format!("NRBF array {object_id} does not contain records")),
        }
    }

    pub fn array_primitives_mut(&mut self, object_id: i32) -> Result<&mut Vec<Primitive>, String> {
        let ObjectValue::Array(array) = &mut self.object_mut(object_id)?.value else {
            return Err(format!("NRBF object {object_id} is not an array"));
        };
        match array {
            ArrayObject::Primitive { values, .. }
            | ArrayObject::Binary {
                values: ArrayValues::Primitive(values),
                ..
            } => Ok(values),
            _ => Err(format!("NRBF array {object_id} is not primitive")),
        }
    }

    pub fn list_records(&self, list_id: i32) -> Result<Vec<Record>, String> {
        let size = usize::try_from(self.field_i64(list_id, "_size")?)
            .map_err(|_| "NRBF list size is negative".to_string())?;
        let array_id = self
            .field_object_id(list_id, "_items")?
            .ok_or_else(|| "NRBF list items are null".to_string())?;
        let mut values = self.array_records(array_id)?;
        if size > values.len() {
            return Err("NRBF list size exceeds backing array".to_string());
        }
        values.truncate(size);
        Ok(values)
    }

    pub fn set_list_records(&mut self, list_id: i32, values: Vec<Record>) -> Result<(), String> {
        if values.len() > MAX_COLLECTION_ITEMS {
            return Err("NRBF list exceeds item limit".to_string());
        }
        let value_count = values.len();
        let array_id = self
            .field_object_id(list_id, "_items")?
            .ok_or_else(|| "NRBF list items are null".to_string())?;
        let ObjectValue::Array(array) = &mut self.object_mut(array_id)?.value else {
            return Err("NRBF list backing object is not an array".to_string());
        };
        match array {
            ArrayObject::Object {
                length,
                values: target,
                ..
            } => {
                *length = values.len();
                *target = values;
            }
            ArrayObject::Binary {
                lengths,
                values: ArrayValues::Records(target),
                ..
            } if lengths.len() == 1 => {
                lengths[0] = i32::try_from(values.len())
                    .map_err(|_| "NRBF list is too large".to_string())?;
                *target = values;
            }
            _ => return Err("NRBF list backing array type is unsupported".to_string()),
        }
        self.set_field_i64(list_id, "_size", i64::try_from(value_count).unwrap())?;
        Ok(())
    }

    pub fn append_object(&mut self, value: ObjectValue) -> Result<i32, String> {
        let id = self.allocate_id()?;
        self.objects.insert(id, Object { id, value });
        let end = self
            .records
            .iter()
            .position(|record| matches!(record, Record::End))
            .ok_or_else(|| "NRBF MessageEnd is missing".to_string())?;
        self.records.insert(end, Record::Object(id));
        Ok(id)
    }

    pub fn add_string(&mut self, value: String) -> Result<i32, String> {
        if value.len() > MAX_STRING_BYTES {
            return Err("NRBF string exceeds size limit".to_string());
        }
        self.append_object(ObjectValue::String(value))
    }

    pub fn add_class(
        &mut self,
        metadata_id: i32,
        fields: BTreeMap<String, FieldValue>,
    ) -> Result<i32, String> {
        let metadata = self
            .metadata
            .get(&metadata_id)
            .cloned()
            .ok_or_else(|| format!("NRBF metadata {metadata_id} is missing"))?;
        let mut ordered = Vec::with_capacity(metadata.members.len());
        for (name, member_type) in metadata.members.iter().zip(&metadata.member_types) {
            ordered.push(fields.get(name).cloned().unwrap_or_else(|| {
                member_type
                    .primitive_kind()
                    .map(|kind| FieldValue::Primitive(zero_primitive(kind)))
                    .unwrap_or(FieldValue::Record(Record::Null))
            }));
        }
        self.append_object(ObjectValue::Class(ClassObject {
            record_type: 1,
            metadata_id,
            metadata: None,
            fields: ordered,
        }))
    }

    pub fn add_enum(&mut self, metadata_id: i32, value: i32) -> Result<i32, String> {
        self.add_class(
            metadata_id,
            BTreeMap::from([(
                "value__".to_string(),
                FieldValue::Primitive(Primitive::int32(value)),
            )]),
        )
    }

    pub fn remove_object_record(&mut self, object_id: i32) {
        self.records
            .retain(|record| !matches!(record, Record::Object(id) if *id == object_id));
        self.objects.remove(&object_id);
    }
    pub fn add_typed_list(
        &mut self,
        metadata_id: i32,
        values: Vec<Record>,
        element_class: &str,
        library_id: i32,
    ) -> Result<i32, String> {
        let length =
            i32::try_from(values.len()).map_err(|_| "NRBF typed list is too large".to_string())?;
        let array_id = self.append_object(ObjectValue::Array(ArrayObject::Binary {
            array_type: 0,
            lengths: vec![length],
            lower_bounds: Vec::new(),
            member_type: MemberType {
                binary_type: 4,
                additional: Some(AdditionalTypeInfo::Class {
                    name: element_class.to_string(),
                    library_id,
                }),
            },
            values: ArrayValues::Records(values),
        }))?;
        self.add_class(
            metadata_id,
            BTreeMap::from([
                (
                    "_items".to_string(),
                    FieldValue::Record(Record::Reference(array_id)),
                ),
                (
                    "_size".to_string(),
                    FieldValue::Primitive(Primitive::int32(length)),
                ),
                (
                    "_version".to_string(),
                    FieldValue::Primitive(Primitive::int32(0)),
                ),
            ]),
        )
    }

    fn allocate_id(&mut self) -> Result<i32, String> {
        while self.objects.contains_key(&self.next_object_id) {
            self.next_object_id = self
                .next_object_id
                .checked_add(1)
                .ok_or_else(|| "NRBF object id space is exhausted".to_string())?;
        }
        let id = self.next_object_id;
        self.next_object_id = self
            .next_object_id
            .checked_add(1)
            .ok_or_else(|| "NRBF object id space is exhausted".to_string())?;
        Ok(id)
    }

    fn write_record(&self, record: &Record, output: &mut Vec<u8>) -> Result<(), String> {
        match record {
            Record::Header {
                root_id,
                header_id,
                major_version,
                minor_version,
            } => {
                output.push(SERIALIZED_STREAM_HEADER);
                put_i32(output, *root_id);
                put_i32(output, *header_id);
                put_i32(output, *major_version);
                put_i32(output, *minor_version);
            }
            Record::Object(id) => self.write_object(*id, output)?,
            Record::Reference(id) => {
                output.push(9);
                put_i32(output, *id);
            }
            Record::Null => output.push(10),
            Record::NullMultiple { count, small } => {
                if *small {
                    output.push(13);
                    output.push(
                        u8::try_from(*count)
                            .map_err(|_| "NRBF small null run overflow".to_string())?,
                    );
                } else {
                    output.push(14);
                    put_i32(
                        output,
                        i32::try_from(*count).map_err(|_| "NRBF null run overflow".to_string())?,
                    );
                }
            }
            Record::PrimitiveTyped(value) => {
                output.push(8);
                output.push(value.kind);
                output.extend_from_slice(&value.data);
            }
            Record::Library { id, name } => {
                output.push(12);
                put_i32(output, *id);
                put_string(output, name)?;
            }
            Record::End => output.push(MESSAGE_END),
        }
        Ok(())
    }

    fn write_object(&self, id: i32, output: &mut Vec<u8>) -> Result<(), String> {
        let object = self.object(id)?;
        match &object.value {
            ObjectValue::String(value) => {
                output.push(BINARY_OBJECT_STRING);
                put_i32(output, id);
                put_string(output, value)?;
            }
            ObjectValue::Class(class) => {
                output.push(class.record_type);
                put_i32(output, id);
                let metadata = self.class_metadata(class)?;
                match class.record_type {
                    1 => put_i32(output, class.metadata_id),
                    2..=5 => {
                        put_string(output, &metadata.name)?;
                        put_i32(
                            output,
                            i32::try_from(metadata.members.len())
                                .map_err(|_| "NRBF member count overflow".to_string())?,
                        );
                        for name in &metadata.members {
                            put_string(output, name)?;
                        }
                        if matches!(class.record_type, 4 | 5) {
                            for member in &metadata.member_types {
                                output.push(member.binary_type);
                            }
                            for member in &metadata.member_types {
                                write_additional(output, member)?;
                            }
                        }
                        if matches!(class.record_type, 3 | 5) {
                            put_i32(
                                output,
                                metadata.library_id.ok_or_else(|| {
                                    "NRBF class library id is missing".to_string()
                                })?,
                            );
                        }
                    }
                    other => return Err(format!("unsupported NRBF class record {other}")),
                }
                if class.fields.len() != metadata.members.len() {
                    return Err(format!("NRBF class {} field count mismatch", metadata.name));
                }
                for (field, member_type) in class.fields.iter().zip(&metadata.member_types) {
                    match (field, member_type.primitive_kind()) {
                        (FieldValue::Primitive(value), Some(kind)) if value.kind == kind => {
                            output.extend_from_slice(&value.data)
                        }
                        (FieldValue::Record(record), None) => self.write_record(record, output)?,
                        _ => {
                            return Err(format!("NRBF class {} field type mismatch", metadata.name))
                        }
                    }
                }
            }
            ObjectValue::Array(array) => self.write_array(id, array, output)?,
        }
        Ok(())
    }

    fn write_array(
        &self,
        id: i32,
        array: &ArrayObject,
        output: &mut Vec<u8>,
    ) -> Result<(), String> {
        match array {
            ArrayObject::Primitive {
                primitive_type,
                values,
            } => {
                output.push(15);
                put_i32(output, id);
                put_i32(
                    output,
                    i32::try_from(values.len())
                        .map_err(|_| "NRBF array length overflow".to_string())?,
                );
                output.push(*primitive_type);
                for value in values {
                    if value.kind != *primitive_type {
                        return Err("NRBF primitive array type mismatch".to_string());
                    }
                    output.extend_from_slice(&value.data);
                }
            }
            ArrayObject::Object {
                string_only,
                length,
                values,
            } => {
                output.push(if *string_only { 17 } else { 16 });
                put_i32(output, id);
                put_i32(
                    output,
                    i32::try_from(*length).map_err(|_| "NRBF array length overflow".to_string())?,
                );
                if values.iter().map(Record::logical_count).sum::<usize>() != *length {
                    return Err("NRBF object array logical length mismatch".to_string());
                }
                for value in values {
                    self.write_record(value, output)?;
                }
            }
            ArrayObject::Binary {
                array_type,
                lengths,
                lower_bounds,
                member_type,
                values,
            } => {
                output.push(7);
                put_i32(output, id);
                output.push(*array_type);
                put_i32(
                    output,
                    i32::try_from(lengths.len())
                        .map_err(|_| "NRBF array rank overflow".to_string())?,
                );
                for length in lengths {
                    put_i32(output, *length);
                }
                if matches!(*array_type, 3..=5) {
                    if lower_bounds.len() != lengths.len() {
                        return Err("NRBF array lower-bound count mismatch".to_string());
                    }
                    for bound in lower_bounds {
                        put_i32(output, *bound);
                    }
                }
                output.push(member_type.binary_type);
                write_additional(output, member_type)?;
                match values {
                    ArrayValues::Primitive(values) => {
                        let kind = member_type.primitive_kind().ok_or_else(|| {
                            "NRBF binary array primitive metadata is missing".to_string()
                        })?;
                        for value in values {
                            if value.kind != kind {
                                return Err("NRBF binary array primitive mismatch".to_string());
                            }
                            output.extend_from_slice(&value.data);
                        }
                    }
                    ArrayValues::Records(values) => {
                        for value in values {
                            self.write_record(value, output)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    objects: BTreeMap<i32, Object>,
    metadata: BTreeMap<i32, ClassMetadata>,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            objects: BTreeMap::new(),
            metadata: BTreeMap::new(),
        }
    }

    fn done(&self) -> bool {
        self.position >= self.bytes.len()
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| "NRBF offset overflow".to_string())?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| "NRBF stream is truncated".to_string())?;
        self.position = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<String, String> {
        let length = self.length_7bit()?;
        if length > MAX_STRING_BYTES {
            return Err("NRBF string exceeds size limit".to_string());
        }
        String::from_utf8(self.take(length)?.to_vec())
            .map_err(|error| format!("NRBF string is not UTF-8: {error}"))
    }

    fn length_7bit(&mut self) -> Result<usize, String> {
        let mut result = 0_usize;
        for shift in (0..35).step_by(7) {
            let byte = self.byte()?;
            result |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
        Err("NRBF 7-bit length is invalid".to_string())
    }

    fn primitive(&mut self, kind: u8) -> Result<Primitive, String> {
        let start = self.position;
        match kind {
            1 | 2 | 10 => {
                self.take(1)?;
            }
            3 => {
                let first = self.byte()?;
                let length = match first {
                    0x00..=0x7f => 1,
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    _ => return Err("NRBF Char has invalid UTF-8".to_string()),
                };
                if length > 1 {
                    self.take(length - 1)?;
                }
            }
            5 => {
                self.take(16)?;
            }
            6 | 9 | 12 | 13 | 16 => {
                self.take(8)?;
            }
            7 | 14 => {
                self.take(2)?;
            }
            8 | 11 | 15 => {
                self.take(4)?;
            }
            17 => {}
            18 => {
                let length = self.length_7bit()?;
                self.take(length)?;
            }
            other => return Err(format!("unsupported NRBF primitive type {other}")),
        }
        Ok(Primitive {
            kind,
            data: self.bytes[start..self.position].to_vec(),
        })
    }

    fn member_type(&mut self, binary_type: u8) -> Result<MemberType, String> {
        let additional = match binary_type {
            0 | 7 => Some(AdditionalTypeInfo::Primitive(self.byte()?)),
            3 => Some(AdditionalTypeInfo::SystemClass(self.string()?)),
            4 => Some(AdditionalTypeInfo::Class {
                name: self.string()?,
                library_id: self.i32()?,
            }),
            1 | 2 | 5 | 6 => None,
            other => return Err(format!("unsupported NRBF binary type {other}")),
        };
        Ok(MemberType {
            binary_type,
            additional,
        })
    }

    fn field(&mut self, member_type: &MemberType) -> Result<FieldValue, String> {
        match member_type.primitive_kind() {
            Some(kind) => Ok(FieldValue::Primitive(self.primitive(kind)?)),
            None => Ok(FieldValue::Record(self.read_record()?)),
        }
    }

    fn read_record(&mut self) -> Result<Record, String> {
        let record_type = self.byte()?;
        match record_type {
            0 => Ok(Record::Header {
                root_id: self.i32()?,
                header_id: self.i32()?,
                major_version: self.i32()?,
                minor_version: self.i32()?,
            }),
            1 => self.read_class_with_id(),
            2..=5 => self.read_class(record_type),
            6 => {
                let id = self.i32()?;
                let value = self.string()?;
                self.insert_object(Object {
                    id,
                    value: ObjectValue::String(value),
                })?;
                Ok(Record::Object(id))
            }
            7 => self.read_binary_array(),
            8 => {
                let kind = self.byte()?;
                Ok(Record::PrimitiveTyped(self.primitive(kind)?))
            }
            9 => Ok(Record::Reference(self.i32()?)),
            10 => Ok(Record::Null),
            11 => Ok(Record::End),
            12 => Ok(Record::Library {
                id: self.i32()?,
                name: self.string()?,
            }),
            13 => Ok(Record::NullMultiple {
                count: usize::from(self.byte()?),
                small: true,
            }),
            14 => Ok(Record::NullMultiple {
                count: usize::try_from(self.i32()?)
                    .map_err(|_| "NRBF null run is negative".to_string())?,
                small: false,
            }),
            15 => self.read_primitive_array(),
            16 | 17 => self.read_object_array(record_type == 17),
            other => Err(format!("unsupported NRBF record type {other}")),
        }
    }

    fn read_class_with_id(&mut self) -> Result<Record, String> {
        let id = self.i32()?;
        let metadata_id = self.i32()?;
        let metadata = self
            .metadata
            .get(&metadata_id)
            .cloned()
            .ok_or_else(|| format!("NRBF metadata {metadata_id} is missing"))?;
        let fields = metadata
            .member_types
            .iter()
            .map(|member_type| self.field(member_type))
            .collect::<Result<Vec<_>, _>>()?;
        self.insert_object(Object {
            id,
            value: ObjectValue::Class(ClassObject {
                record_type: 1,
                metadata_id,
                metadata: None,
                fields,
            }),
        })?;
        Ok(Record::Object(id))
    }

    fn read_class(&mut self, record_type: u8) -> Result<Record, String> {
        let id = self.i32()?;
        let name = self.string()?;
        let member_count = usize::try_from(self.i32()?)
            .map_err(|_| "NRBF class member count is negative".to_string())?;
        if member_count > MAX_COLLECTION_ITEMS {
            return Err("NRBF class has too many members".to_string());
        }
        let members = (0..member_count)
            .map(|_| self.string())
            .collect::<Result<Vec<_>, _>>()?;
        let member_types = if matches!(record_type, 4 | 5) {
            let binary_types = self.take(member_count)?.to_vec();
            binary_types
                .into_iter()
                .map(|binary_type| self.member_type(binary_type))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![
                MemberType {
                    binary_type: 2,
                    additional: None
                };
                member_count
            ]
        };
        let library_id = if matches!(record_type, 3 | 5) {
            Some(self.i32()?)
        } else {
            None
        };
        let metadata = ClassMetadata {
            object_id: id,
            name,
            members,
            member_types,
            library_id,
        };
        self.metadata.insert(id, metadata.clone());
        let fields = metadata
            .member_types
            .iter()
            .map(|member_type| self.field(member_type))
            .collect::<Result<Vec<_>, _>>()?;
        self.insert_object(Object {
            id,
            value: ObjectValue::Class(ClassObject {
                record_type,
                metadata_id: id,
                metadata: Some(metadata),
                fields,
            }),
        })?;
        Ok(Record::Object(id))
    }

    fn read_primitive_array(&mut self) -> Result<Record, String> {
        let id = self.i32()?;
        let length = self.collection_length()?;
        let primitive_type = self.byte()?;
        let values = (0..length)
            .map(|_| self.primitive(primitive_type))
            .collect::<Result<Vec<_>, _>>()?;
        self.insert_object(Object {
            id,
            value: ObjectValue::Array(ArrayObject::Primitive {
                primitive_type,
                values,
            }),
        })?;
        Ok(Record::Object(id))
    }

    fn read_object_array(&mut self, string_only: bool) -> Result<Record, String> {
        let id = self.i32()?;
        let length = self.collection_length()?;
        let values = self.record_values(length)?;
        self.insert_object(Object {
            id,
            value: ObjectValue::Array(ArrayObject::Object {
                string_only,
                length,
                values,
            }),
        })?;
        Ok(Record::Object(id))
    }

    fn read_binary_array(&mut self) -> Result<Record, String> {
        let id = self.i32()?;
        let array_type = self.byte()?;
        let rank = self.collection_length()?;
        let lengths = (0..rank)
            .map(|_| self.i32())
            .collect::<Result<Vec<_>, _>>()?;
        let lower_bounds = if matches!(array_type, 3..=5) {
            (0..rank)
                .map(|_| self.i32())
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        let binary_type = self.byte()?;
        let member_type = self.member_type(binary_type)?;
        let logical_length = lengths.iter().try_fold(1_usize, |total, length| {
            let length = usize::try_from(*length)
                .map_err(|_| "NRBF array length is negative".to_string())?;
            total
                .checked_mul(length)
                .ok_or_else(|| "NRBF array length overflow".to_string())
        })?;
        if logical_length > MAX_COLLECTION_ITEMS {
            return Err("NRBF array exceeds item limit".to_string());
        }
        let values = match member_type.primitive_kind() {
            Some(kind) => ArrayValues::Primitive(
                (0..logical_length)
                    .map(|_| self.primitive(kind))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            None => ArrayValues::Records(self.record_values(logical_length)?),
        };
        self.insert_object(Object {
            id,
            value: ObjectValue::Array(ArrayObject::Binary {
                array_type,
                lengths,
                lower_bounds,
                member_type,
                values,
            }),
        })?;
        Ok(Record::Object(id))
    }

    fn record_values(&mut self, logical_length: usize) -> Result<Vec<Record>, String> {
        let mut values = Vec::new();
        let mut count = 0_usize;
        while count < logical_length {
            let record = self.read_record()?;
            count = count
                .checked_add(record.logical_count())
                .ok_or_else(|| "NRBF array count overflow".to_string())?;
            if count > logical_length {
                return Err("NRBF null run exceeds array length".to_string());
            }
            values.push(record);
        }
        Ok(values)
    }

    fn collection_length(&mut self) -> Result<usize, String> {
        let length = usize::try_from(self.i32()?)
            .map_err(|_| "NRBF collection length is negative".to_string())?;
        if length > MAX_COLLECTION_ITEMS {
            return Err("NRBF collection exceeds item limit".to_string());
        }
        Ok(length)
    }

    fn insert_object(&mut self, object: Object) -> Result<(), String> {
        if self.objects.insert(object.id, object).is_some() {
            return Err("NRBF object id is defined twice".to_string());
        }
        Ok(())
    }
}

fn zero_primitive(kind: u8) -> Primitive {
    let length = match kind {
        1 | 2 | 10 => 1,
        3 => 1,
        5 => 16,
        6 | 9 | 12 | 13 | 16 => 8,
        7 | 14 => 2,
        8 | 11 | 15 => 4,
        17 => 0,
        18 => 1,
        _ => 0,
    };
    Primitive {
        kind,
        data: vec![0; length],
    }
}

fn write_additional(output: &mut Vec<u8>, member: &MemberType) -> Result<(), String> {
    match (&member.additional, member.binary_type) {
        (Some(AdditionalTypeInfo::Primitive(kind)), 0 | 7) => output.push(*kind),
        (Some(AdditionalTypeInfo::SystemClass(name)), 3) => put_string(output, name)?,
        (Some(AdditionalTypeInfo::Class { name, library_id }), 4) => {
            put_string(output, name)?;
            put_i32(output, *library_id);
        }
        (None, 1 | 2 | 5 | 6) => {}
        _ => return Err("NRBF additional type info mismatch".to_string()),
    }
    Ok(())
}

fn put_i32(output: &mut Vec<u8>, value: i32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_string(output: &mut Vec<u8>, value: &str) -> Result<(), String> {
    if value.len() > MAX_STRING_BYTES {
        return Err("NRBF string exceeds size limit".to_string());
    }
    put_7bit(output, value.len());
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn put_7bit(output: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_writes_a_minimal_stream_exactly() {
        let bytes = [
            0, 1, 0, 0, 0, 255, 255, 255, 255, 1, 0, 0, 0, 0, 0, 0, 0, 6, 1, 0, 0, 0, 2, b'o',
            b'k', 11,
        ];
        let document = Document::parse(&bytes).unwrap();
        assert_eq!(document.root_id().unwrap(), 1);
        assert_eq!(document.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn appended_strings_remain_valid_unreferenced_records() {
        let bytes = [
            0, 1, 0, 0, 0, 255, 255, 255, 255, 1, 0, 0, 0, 0, 0, 0, 0, 6, 1, 0, 0, 0, 2, b'o',
            b'k', 11,
        ];
        let mut document = Document::parse(&bytes).unwrap();
        let id = document.add_string("native".to_string()).unwrap();
        let reparsed = Document::parse(&document.to_bytes().unwrap()).unwrap();
        assert!(
            matches!(&reparsed.object(id).unwrap().value, ObjectValue::String(value) if value == "native")
        );
    }
}
