using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;

if (args.Length != 2)
{
    Console.Error.WriteLine("usage: ExternalConsumer <win32.winmd> <winrt.winmd>");
    return 2;
}

foreach (string path in args)
{
    using FileStream stream = File.OpenRead(path);
    using PEReader pe = new(stream);
    MetadataReader reader = pe.GetMetadataReader();
    List<string> fingerprint = [$"kind:{reader.MetadataKind}"];

    foreach (TypeDefinitionHandle handle in reader.TypeDefinitions)
    {
        TypeDefinition type = reader.GetTypeDefinition(handle);
        string ns = reader.GetString(type.Namespace);
        if (ns != "Test")
        {
            continue;
        }

        string name = reader.GetString(type.Name);
        string fullName = $"{ns}.{name}";
        fingerprint.Add($"type:{fullName}");

        foreach (FieldDefinitionHandle fieldHandle in type.GetFields())
        {
            FieldDefinition field = reader.GetFieldDefinition(fieldHandle);
            fingerprint.Add($"field:{fullName}.{reader.GetString(field.Name)}");
        }

        foreach (MethodDefinitionHandle methodHandle in type.GetMethods())
        {
            MethodDefinition method = reader.GetMethodDefinition(methodHandle);
            fingerprint.Add($"method:{fullName}.{reader.GetString(method.Name)}");
        }
    }

    fingerprint.Sort(StringComparer.Ordinal);
    Console.WriteLine(Path.GetFileName(path));
    foreach (string line in fingerprint)
    {
        Console.WriteLine(line);
    }
}

return 0;
